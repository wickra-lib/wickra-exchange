package org.wickra.exchange;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.file.Path;

/** Raw FFM (Panama) downcall surface for the wickra-exchange C ABI. */
final class Native {
    private Native() {}

    static final int OK = 0;
    static final int SIDE_BUY = 0;
    static final int SIDE_SELL = 1;

    static final int STATUS_NEW = 0;
    static final int STATUS_FILLED = 2;

    static final int EVENT_TRADE = 0;
    static final int EVENT_ORDER_UPDATE = 2;

    static final int STR_CAP = 64;

    static final int MARGIN_CROSS = 0;
    static final int MARGIN_ISOLATED = 1;
    static final int POSITION_LONG = 0;
    static final int POSITION_SHORT = 1;

    // WickraPosition field offsets (repr(C), 8-aligned; total 120 bytes).
    static final long POSITION_SIZE = 120;
    static final long P_SYMBOL = 0;
    static final long P_SIDE = 64;
    static final long P_QUANTITY = 72;
    static final long P_ENTRY = 80;
    static final long P_MARK = 88;
    static final long P_LEVERAGE = 96;
    static final long P_UPNL = 104;
    static final long P_MARGIN_MODE = 112;

    // WickraOrder field offsets (repr(C), 8-aligned; total 104 bytes).
    static final long ORDER_SIZE = 104;
    static final long O_ID = 0;
    static final long O_SIDE = 64;
    static final long O_STATUS = 68;
    static final long O_QUANTITY = 72;
    static final long O_FILLED = 80;
    static final long O_PRICE = 88;
    static final long O_AVG = 96;

    // WickraEvent field offsets (repr(C), 8-aligned; total 200 bytes).
    static final long EVENT_SIZE = 200;
    static final long E_KIND = 0;
    static final long E_SYMBOL = 4;
    static final long E_PRICE = 72;
    static final long E_QUANTITY = 80;
    static final long E_SIDE = 88;
    static final long E_ORDER = 96;

    private static final Linker LINKER = Linker.nativeLinker();
    private static final Arena LIB_ARENA = Arena.ofShared();
    private static final SymbolLookup LOOKUP = loadLibrary();

    static final ValueLayout.OfInt C_INT = ValueLayout.JAVA_INT;
    static final ValueLayout.OfDouble C_DOUBLE = ValueLayout.JAVA_DOUBLE;
    static final ValueLayout.OfLong C_SIZE = ValueLayout.JAVA_LONG;
    static final ValueLayout.OfByte C_BOOL = ValueLayout.JAVA_BYTE;
    static final java.lang.foreign.AddressLayout C_PTR = ValueLayout.ADDRESS;

    static final MethodHandle VERSION =
            handle("wickra_version", FunctionDescriptor.of(C_PTR));
    static final MethodHandle PAPER_NEW = handle("wickra_paper_new",
            FunctionDescriptor.of(C_PTR, C_PTR, C_PTR, C_SIZE, C_DOUBLE, C_DOUBLE, C_DOUBLE));
    static final MethodHandle REPLAY_NEW = handle("wickra_replay_new",
            FunctionDescriptor.of(C_PTR, C_PTR, C_PTR, C_SIZE, C_PTR, C_PTR, C_SIZE, C_DOUBLE, C_DOUBLE, C_DOUBLE));
    static final MethodHandle CONNECT = handle("wickra_connect",
            FunctionDescriptor.of(C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_BOOL));
    static final MethodHandle FREE = handle("wickra_exchange_free",
            FunctionDescriptor.ofVoid(C_PTR));
    static final MethodHandle NAME = handle("wickra_exchange_name",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_SIZE));
    static final MethodHandle SET_PRICE = handle("wickra_exchange_set_price",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_DOUBLE));
    static final MethodHandle PLACE_MARKET = handle("wickra_exchange_place_market",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_INT, C_DOUBLE, C_PTR));
    static final MethodHandle PLACE_LIMIT = handle("wickra_exchange_place_limit",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_INT, C_DOUBLE, C_DOUBLE, C_PTR));
    static final MethodHandle CANCEL = handle("wickra_exchange_cancel",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR));
    static final MethodHandle BALANCE = handle("wickra_exchange_balance",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR));
    static final MethodHandle POLL = handle("wickra_exchange_poll",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_SIZE));

    static final MethodHandle CONNECT_DERIVATIVES = handle("wickra_connect_derivatives",
            FunctionDescriptor.of(C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_BOOL));
    static final MethodHandle DERIVATIVES_FREE = handle("wickra_derivatives_free",
            FunctionDescriptor.ofVoid(C_PTR));
    static final MethodHandle DERIVATIVES_POSITION = handle("wickra_derivatives_position",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR));
    static final MethodHandle DERIVATIVES_POSITIONS = handle("wickra_derivatives_positions",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR, C_SIZE));
    static final MethodHandle DERIVATIVES_SET_LEVERAGE = handle("wickra_derivatives_set_leverage",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_INT));
    static final MethodHandle DERIVATIVES_SET_MARGIN_MODE = handle("wickra_derivatives_set_margin_mode",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_INT));
    static final MethodHandle DERIVATIVES_CLOSE_POSITION = handle("wickra_derivatives_close_position",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR));

    static final MethodHandle CONNECT_ADVANCED = handle("wickra_connect_advanced",
            FunctionDescriptor.of(C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_BOOL, C_BOOL));
    static final MethodHandle ADVANCED_FREE = handle("wickra_advanced_free",
            FunctionDescriptor.ofVoid(C_PTR));
    static final MethodHandle ADVANCED_AMEND_ORDER = handle("wickra_advanced_amend_order",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR, C_DOUBLE, C_DOUBLE, C_PTR));
    static final MethodHandle ADVANCED_CANCEL_BATCH = handle("wickra_advanced_cancel_batch",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR, C_SIZE));
    static final MethodHandle ADVANCED_PLACE_OCO = handle("wickra_advanced_place_oco",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_INT, C_DOUBLE, C_DOUBLE, C_DOUBLE, C_DOUBLE, C_PTR, C_SIZE));
    static final MethodHandle ADVANCED_PLACE_BATCH = handle("wickra_advanced_place_batch",
            FunctionDescriptor.of(C_INT, C_PTR, C_PTR, C_PTR, C_PTR, C_PTR, C_SIZE, C_PTR, C_PTR, C_SIZE));

    private static SymbolLookup loadLibrary() {
        String dir = System.getProperty("native.lib.dir");
        String libFile = System.mapLibraryName("wickra_exchange");
        Path path = dir != null ? Path.of(dir, libFile) : Path.of(libFile);
        return SymbolLookup.libraryLookup(path, LIB_ARENA);
    }

    private static MethodHandle handle(String name, FunctionDescriptor descriptor) {
        MemorySegment symbol = LOOKUP.find(name)
                .orElseThrow(() -> new IllegalStateException("missing C ABI symbol: " + name));
        return LINKER.downcallHandle(symbol, descriptor);
    }

    /** Read a NUL-terminated C string from a fixed-size field of a struct. */
    static String readCString(MemorySegment segment, long offset, int cap) {
        byte[] bytes = new byte[cap];
        MemorySegment.copy(segment, ValueLayout.JAVA_BYTE, offset, bytes, 0, cap);
        int end = 0;
        while (end < cap && bytes[end] != 0) {
            end++;
        }
        return new String(bytes, 0, end, java.nio.charset.StandardCharsets.UTF_8);
    }
}
