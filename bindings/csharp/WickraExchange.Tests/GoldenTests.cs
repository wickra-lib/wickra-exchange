using System.Text.Json;
using WickraExchange;
using Xunit;

namespace WickraExchange.Tests;

/// <summary>
/// Golden-fixture parity for the C# binding.
///
/// The Rust suite (<c>crates/wickra-exchange-core/tests/golden.rs</c>) drives the
/// committed replay tapes in <c>golden/</c> through a <c>ReplayExchange</c> running a
/// fixed SMA strategy, and pins the fill price and resulting balances. This runs
/// the same fixtures through the same pipeline over the C ABI.
///
/// <see cref="ExchangeTests"/> already proves a paper order fills. What it does not
/// check are the numbers a <em>replayed</em> tape produces: a lost decimal, a dropped
/// fee or slippage applied to the wrong side would still fill, and still pass.
///
/// The strategy is reimplemented rather than imported, so this tests the
/// replay-to-paper-fill pipeline rather than two libraries agreeing.
/// </summary>
public class GoldenTests
{
    private const double Tol = 1e-6;

    /// <summary>
    /// The repository's <c>golden/</c> directory, found by walking up from the test
    /// assembly. The build output sits several levels below the repository root and
    /// the depth differs between configurations, so the directory is searched for
    /// rather than reached by a counted number of "..".
    /// </summary>
    private static string GoldenDir()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            var candidate = Path.Combine(dir.FullName, "golden");
            if (Directory.Exists(candidate))
            {
                return candidate;
            }
            dir = dir.Parent;
        }
        throw new DirectoryNotFoundException("no golden/ directory above " + AppContext.BaseDirectory);
    }

    private static JsonElement Read(string kind, string name)
    {
        var path = Path.Combine(GoldenDir(), kind, name + ".json");
        return JsonDocument.Parse(File.ReadAllText(path)).RootElement.Clone();
    }

    /// A streaming simple moving average; null until it has `window` values.
    private static Func<double, double?> Sma(int window)
    {
        var values = new List<double>();
        return price =>
        {
            values.Add(price);
            if (values.Count > window)
            {
                values.RemoveAt(0);
            }
            return values.Count < window ? null : values.Sum() / window;
        };
    }

    private static void RunCase(string name)
    {
        var spec = Read("replay", name);
        var expected = Read("expected", name);

        var market = spec.GetProperty("market").GetString()!;
        var tape = spec.GetProperty("tape").EnumerateArray().Select(v => v.GetDouble()).ToList();
        var balances = spec.GetProperty("balances").EnumerateObject()
            .ToDictionary(p => p.Name, p => p.Value.GetDouble());

        using var ex = Exchange.ReplayTrades(
            market, tape, balances,
            makerBps: spec.GetProperty("maker_bps").GetDouble(),
            takerBps: spec.GetProperty("taker_bps").GetDouble(),
            slippageBps: spec.GetProperty("slippage_bps").GetDouble());

        var sma = Sma(spec.GetProperty("sma_period").GetInt32());
        double? fillPrice = null;

        // Each poll advances the recording by exactly one frame; an empty batch is
        // how an exhausted tape reports itself.
        while (true)
        {
            var events = ex.Poll(64);
            if (events.Count == 0)
            {
                break;
            }
            foreach (var e in events)
            {
                if (!e.IsTrade || e.Price is null)
                {
                    continue;
                }
                var mean = sma(e.Price.Value);
                if (mean is not null && fillPrice is null && e.Price.Value > mean.Value)
                {
                    fillPrice = ex.PlaceMarket(market, Side.Buy, 1.0).AveragePrice;
                }
            }
        }

        Assert.Equal(expected.GetProperty("filled").GetBoolean(), fillPrice is not null);
        Assert.True(Math.Abs(fillPrice!.Value - expected.GetProperty("average_price").GetDouble()) < Tol,
            $"{name}: average price {fillPrice}");
        Assert.True(Math.Abs(ex.Balance("BTC") - expected.GetProperty("btc").GetDouble()) < Tol,
            $"{name}: BTC {ex.Balance("BTC")}");
        Assert.True(Math.Abs(ex.Balance("USDT") - expected.GetProperty("usdt").GetDouble()) < Tol,
            $"{name}: USDT {ex.Balance("USDT")}");
    }

    [Fact]
    public void GoldenSmaCrossFrictionless() => RunCase("sma_cross");

    [Fact]
    public void GoldenSmaCrossWithCosts() => RunCase("sma_cross_with_costs");
}
