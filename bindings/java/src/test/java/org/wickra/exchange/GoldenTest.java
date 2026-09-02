package org.wickra.exchange;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;

/**
 * Golden-fixture parity for the Java binding.
 *
 * <p>The Rust suite ({@code crates/wickra-exchange-core/tests/golden.rs}) drives the
 * committed replay tapes in {@code golden/} through a {@code ReplayExchange} running a
 * fixed SMA strategy, and pins the fill price and the resulting balances. This runs the
 * same fixtures through the same pipeline over the C ABI.
 *
 * <p>{@link ExchangeTest} already proves a paper order fills. What it does not check are
 * the numbers a <em>replayed</em> tape produces: a lost decimal, a dropped fee or
 * slippage applied to the wrong side would still fill, and still pass.
 *
 * <p>The fixtures are read with the small field reader below rather than a JSON library.
 * The module's only test dependency is JUnit, and taking on a second one so a test can
 * read four numbers and one array out of a file whose shape is fixed and committed would
 * be a poor trade. The reader handles that shape and nothing else, which is why it lives
 * in this test rather than in the main source set.
 */
class GoldenTest {

    private static final double TOL = 1e-6;

    /**
     * The repository's {@code golden/} directory, found by walking up from the working
     * directory. Surefire runs from the module directory, but the depth to the repository
     * root differs between a module build and a reactor build, so it is searched for
     * rather than reached by a counted number of "..".
     */
    private static Path goldenDir() {
        Path dir = Path.of("").toAbsolutePath();
        while (dir != null) {
            Path candidate = dir.resolve("golden");
            if (Files.isDirectory(candidate)) {
                return candidate;
            }
            dir = dir.getParent();
        }
        throw new IllegalStateException("no golden/ directory above " + Path.of("").toAbsolutePath());
    }

    private static String read(String kind, String name) {
        try {
            return Files.readString(goldenDir().resolve(kind).resolve(name + ".json"));
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /** The scalar value of {@code "<key>": <number>}. */
    private static double num(String text, String key) {
        Matcher m = Pattern.compile("\"" + key + "\"\\s*:\\s*(-?[0-9.]+)").matcher(text);
        assertTrue(m.find(), "no numeric field " + key);
        return Double.parseDouble(m.group(1));
    }

    /** The array of numbers at {@code "<key>": [ ... ]}. */
    private static double[] nums(String text, String key) {
        Matcher m = Pattern.compile("\"" + key + "\"\\s*:\\s*\\[([^]]*)]").matcher(text);
        assertTrue(m.find(), "no array field " + key);
        List<Double> out = new ArrayList<>();
        for (String part : m.group(1).split(",")) {
            out.add(Double.parseDouble(part.trim()));
        }
        double[] values = new double[out.size()];
        for (int i = 0; i < values.length; i++) {
            values[i] = out.get(i);
        }
        return values;
    }

    /** The boolean value of {@code "<key>": true|false}. */
    private static boolean bool(String text, String key) {
        Matcher m = Pattern.compile("\"" + key + "\"\\s*:\\s*(true|false)").matcher(text);
        assertTrue(m.find(), "no boolean field " + key);
        return Boolean.parseBoolean(m.group(1));
    }

    /** A streaming simple moving average; null until it holds {@code window} values. */
    private static final class Sma {
        private final int window;
        private final List<Double> values = new ArrayList<>();

        Sma(int window) {
            this.window = window;
        }

        Double update(double price) {
            values.add(price);
            if (values.size() > window) {
                values.remove(0);
            }
            if (values.size() < window) {
                return null;
            }
            double sum = 0.0;
            for (double v : values) {
                sum += v;
            }
            return sum / window;
        }
    }

    private static void runCase(String name) {
        String spec = read("replay", name);
        String expected = read("expected", name);

        Map<String, Double> balances = new HashMap<>();
        balances.put("USDT", num(spec, "USDT"));

        try (Exchange ex = Exchange.replayTrades(
                "BTC/USDT", nums(spec, "tape"), balances,
                num(spec, "maker_bps"), num(spec, "taker_bps"), num(spec, "slippage_bps"))) {

            Sma sma = new Sma((int) num(spec, "sma_period"));
            Double fillPrice = null;

            // Each poll advances the recording by exactly one frame; an empty batch is
            // how an exhausted tape reports itself.
            while (true) {
                List<Exchange.Event> events = ex.poll(64);
                if (events.isEmpty()) {
                    break;
                }
                for (Exchange.Event event : events) {
                    if (event.kind() != Exchange.Kind.TRADE || event.price() == null) {
                        continue;
                    }
                    Double mean = sma.update(event.price());
                    if (mean != null && fillPrice == null && event.price() > mean) {
                        fillPrice = ex.placeMarket("BTC/USDT", Exchange.Side.BUY, 1.0).averagePrice();
                    }
                }
            }

            assertEquals(bool(expected, "filled"), fillPrice != null);
            assertTrue(Math.abs(fillPrice - num(expected, "average_price")) < TOL,
                    name + ": average price " + fillPrice);
            assertTrue(Math.abs(ex.balance("BTC") - num(expected, "btc")) < TOL,
                    name + ": BTC " + ex.balance("BTC"));
            assertTrue(Math.abs(ex.balance("USDT") - num(expected, "usdt")) < TOL,
                    name + ": USDT " + ex.balance("USDT"));
        }
    }

    @Test
    void goldenSmaCrossFrictionless() {
        runCase("sma_cross");
    }

    @Test
    void goldenSmaCrossWithCosts() {
        runCase("sma_cross_with_costs");
    }
}
