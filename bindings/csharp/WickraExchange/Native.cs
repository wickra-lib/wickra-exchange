using System.Runtime.InteropServices;

namespace WickraExchange;

/// <summary>Raw P/Invoke surface for the wickra-exchange C ABI.</summary>
internal static unsafe class Native
{
    private const string Lib = "wickra_exchange";

    public const int Ok = 0;
    public const int SideBuy = 0;
    public const int SideSell = 1;

    public const int StatusNew = 0;
    public const int StatusPartiallyFilled = 1;
    public const int StatusFilled = 2;
    public const int StatusCanceled = 3;
    public const int StatusRejected = 4;
    public const int StatusExpired = 5;

    public const int EventTrade = 0;
    public const int EventTicker = 1;
    public const int EventOrderUpdate = 2;
    public const int EventBalanceUpdate = 3;
    public const int EventSubscribed = 4;
    public const int EventOther = 5;

    public const int MarginCross = 0;
    public const int MarginIsolated = 1;

    public const int PositionLong = 0;
    public const int PositionShort = 1;

    public const int StrCap = 64;

    [StructLayout(LayoutKind.Sequential)]
    public struct Order
    {
        public fixed byte Id[StrCap];
        public int Side;
        public int Status;
        public double Quantity;
        public double FilledQuantity;
        public double Price;
        public double AveragePrice;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Event
    {
        public int Kind;
        public fixed byte Symbol[StrCap];
        public double Price;
        public double Quantity;
        public int Side;
        public Order Order;
    }

    [DllImport(Lib)]
    public static extern nint wickra_version();

    [DllImport(Lib)]
    public static extern nint wickra_paper_new(
        nint* assets, double* amounts, nuint nBalances, double makerBps, double takerBps, double slippageBps);

    [DllImport(Lib)]
    public static extern nint wickra_replay_new(
        byte* market, double* tape, nuint nTape,
        nint* assets, double* amounts, nuint nBalances,
        double makerBps, double takerBps, double slippageBps);

    [DllImport(Lib)]
    public static extern nint wickra_connect(
        byte* name, byte* apiKey, byte* apiSecret, byte* passphrase, byte* privateKey,
        [MarshalAs(UnmanagedType.U1)] bool testnet);

    [DllImport(Lib)]
    public static extern void wickra_exchange_free(nint handle);

    [DllImport(Lib)]
    public static extern int wickra_exchange_name(nint handle, byte* outBuf, nuint cap);

    [DllImport(Lib)]
    public static extern int wickra_exchange_set_price(nint handle, byte* market, double price);

    [DllImport(Lib)]
    public static extern int wickra_exchange_place_market(
        nint handle, byte* market, int side, double quantity, Order* outOrder);

    [DllImport(Lib)]
    public static extern int wickra_exchange_place_limit(
        nint handle, byte* market, int side, double quantity, double price, Order* outOrder);

    [DllImport(Lib)]
    public static extern int wickra_exchange_cancel(nint handle, byte* market, byte* orderId);

    [DllImport(Lib)]
    public static extern int wickra_exchange_balance(nint handle, byte* asset, double* outFree);

    [DllImport(Lib)]
    public static extern int wickra_exchange_poll(nint handle, Event* outBuf, nuint cap);

    [StructLayout(LayoutKind.Sequential)]
    public struct Position
    {
        public fixed byte Symbol[StrCap];
        public int Side;
        public double Quantity;
        public double EntryPrice;
        public double MarkPrice;
        public double Leverage;
        public double UnrealizedPnl;
        public int MarginMode;
    }

    [DllImport(Lib)]
    public static extern nint wickra_connect_derivatives(
        byte* name, byte* apiKey, byte* apiSecret, byte* passphrase, byte* privateKey,
        [MarshalAs(UnmanagedType.U1)] bool testnet);

    [DllImport(Lib)]
    public static extern void wickra_derivatives_free(nint handle);

    [DllImport(Lib)]
    public static extern int wickra_derivatives_position(nint handle, byte* market, Position* outPos);

    [DllImport(Lib)]
    public static extern int wickra_derivatives_positions(nint handle, byte* market, Position* outBuf, nuint cap);

    [DllImport(Lib)]
    public static extern int wickra_derivatives_set_leverage(nint handle, byte* market, uint leverage);

    [DllImport(Lib)]
    public static extern int wickra_derivatives_set_margin_mode(nint handle, byte* market, int mode);

    [DllImport(Lib)]
    public static extern int wickra_derivatives_close_position(nint handle, byte* market, Order* outOrder);

    [DllImport(Lib)]
    public static extern nint wickra_connect_advanced(
        byte* name, byte* apiKey, byte* apiSecret, byte* passphrase, byte* privateKey,
        [MarshalAs(UnmanagedType.U1)] bool testnet, [MarshalAs(UnmanagedType.U1)] bool futures);

    [DllImport(Lib)]
    public static extern void wickra_advanced_free(nint handle);

    [DllImport(Lib)]
    public static extern int wickra_advanced_amend_order(
        nint handle, byte* market, byte* orderId, double newPrice, double newQuantity, Order* outOrder);

    [DllImport(Lib)]
    public static extern int wickra_advanced_cancel_batch(nint handle, byte* market, nint* orderIds, nuint n);

    [DllImport(Lib)]
    public static extern int wickra_advanced_place_oco(
        nint handle, byte* market, int side, double quantity, double price, double stopPrice,
        double stopLimitPrice, Order* outBuf, nuint cap);

    [DllImport(Lib)]
    public static extern int wickra_advanced_place_batch(
        nint handle, nint* markets, int* sides, double* quantities, double* prices, nuint n,
        Order* outBuf, int* outCodes, nuint cap);

    [DllImport(Lib)]
    public static extern nint wickra_connect_user_data(
        byte* name, byte* apiKey, byte* apiSecret, byte* passphrase, byte* privateKey,
        [MarshalAs(UnmanagedType.U1)] bool testnet, [MarshalAs(UnmanagedType.U1)] bool futures);

    [DllImport(Lib)]
    public static extern void wickra_user_data_free(nint handle);

    [DllImport(Lib)]
    public static extern int wickra_user_data_subscribe(nint handle);

    [DllImport(Lib)]
    public static extern int wickra_user_data_poll(nint handle, Event* outBuf, nuint cap);

    [DllImport(Lib)]
    public static extern nint wickra_connect_ws_execution(
        byte* name, byte* apiKey, byte* apiSecret, byte* passphrase, byte* privateKey,
        [MarshalAs(UnmanagedType.U1)] bool testnet, [MarshalAs(UnmanagedType.U1)] bool futures);

    [DllImport(Lib)]
    public static extern void wickra_ws_execution_free(nint handle);

    [DllImport(Lib)]
    public static extern int wickra_ws_place_order(
        nint handle, byte* market, int side, double quantity, double price, Order* outOrder);

    [DllImport(Lib)]
    public static extern int wickra_ws_cancel_order(nint handle, byte* market, byte* orderId);
}
