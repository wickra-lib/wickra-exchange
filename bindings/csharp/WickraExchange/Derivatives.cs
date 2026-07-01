using System.Runtime.InteropServices;

namespace WickraExchange;

/// <summary>The margin mode of a derivatives position.</summary>
public enum MarginMode
{
    Cross = Native.MarginCross,
    Isolated = Native.MarginIsolated,
}

/// <summary>The direction of a position.</summary>
public enum PositionSide
{
    Long = Native.PositionLong,
    Short = Native.PositionShort,
}

/// <summary>An open derivatives position.</summary>
public sealed record PositionInfo(
    string Symbol, PositionSide Side, double Quantity, double EntryPrice,
    double MarkPrice, double Leverage, double UnrealizedPnl, MarginMode MarginMode);

/// <summary>
/// A live derivatives (futures/perpetual) client: positions, leverage, margin
/// mode and reduce-only close. Construct with <see cref="Connect"/>. Available on
/// the eight venues with futures markets.
/// </summary>
public sealed unsafe class Derivatives : IDisposable
{
    private nint _handle;

    private Derivatives(nint handle) => _handle = handle;

    /// <summary>Connect a USDⓈ-M futures client for <paramref name="name"/>.</summary>
    public static Derivatives Connect(
        string name, string apiKey, string apiSecret,
        string? passphrase = null, string? privateKey = null, bool testnet = false)
    {
        nint pass = passphrase is null ? 0 : Marshal.StringToCoTaskMemUTF8(passphrase);
        nint priv = privateKey is null ? 0 : Marshal.StringToCoTaskMemUTF8(privateKey);
        var nameBytes = Exchange.Utf8(name);
        var keyBytes = Exchange.Utf8(apiKey);
        var secretBytes = Exchange.Utf8(apiSecret);
        try
        {
            fixed (byte* np = nameBytes)
            fixed (byte* kp = keyBytes)
            fixed (byte* sp = secretBytes)
            {
                nint handle = Native.wickra_connect_derivatives(np, kp, sp, (byte*)pass, (byte*)priv, testnet);
                if (handle == 0)
                {
                    throw new WickraException($"failed to connect derivatives client for {name}");
                }
                return new Derivatives(handle);
            }
        }
        finally
        {
            if (pass != 0) { Marshal.FreeCoTaskMem(pass); }
            if (priv != 0) { Marshal.FreeCoTaskMem(priv); }
        }
    }

    /// <summary>The open position in <paramref name="market"/> (throws if flat).</summary>
    public PositionInfo Position(string market)
    {
        var m = Exchange.Utf8(market);
        Native.Position pos;
        fixed (byte* mp = m)
        {
            Exchange.Check(Native.wickra_derivatives_position(_handle, mp, &pos));
        }
        return Exchange.ReadPosition(pos);
    }

    /// <summary>
    /// Every open position. Pass a <paramref name="market"/> to scope to one
    /// symbol, or <c>null</c> for all. Grows its buffer and re-queries if the
    /// venue reports more positions than fit.
    /// </summary>
    public IReadOnlyList<PositionInfo> Positions(string? market = null)
    {
        byte[]? m = market is null ? null : Exchange.Utf8(market);
        int cap = 16;
        while (true)
        {
            var buffer = new Native.Position[cap];
            int count;
            fixed (byte* mp = m)
            fixed (Native.Position* bp = buffer)
            {
                count = Native.wickra_derivatives_positions(_handle, mp, bp, (nuint)cap);
            }
            if (count < 0)
            {
                throw new WickraException($"positions failed with code {count}");
            }
            if (count > cap)
            {
                cap = count;
                continue;
            }
            var result = new List<PositionInfo>(count);
            for (int i = 0; i < count; i++)
            {
                result.Add(Exchange.ReadPosition(buffer[i]));
            }
            return result;
        }
    }

    /// <summary>Set the leverage for <paramref name="market"/>.</summary>
    public void SetLeverage(string market, uint leverage)
    {
        var m = Exchange.Utf8(market);
        fixed (byte* mp = m)
        {
            Exchange.Check(Native.wickra_derivatives_set_leverage(_handle, mp, leverage));
        }
    }

    /// <summary>Set the margin mode for <paramref name="market"/>.</summary>
    public void SetMarginMode(string market, MarginMode mode)
    {
        var m = Exchange.Utf8(market);
        fixed (byte* mp = m)
        {
            Exchange.Check(Native.wickra_derivatives_set_margin_mode(_handle, mp, (int)mode));
        }
    }

    /// <summary>Flatten the open position in <paramref name="market"/> (reduce-only market order).</summary>
    public OrderInfo ClosePosition(string market)
    {
        var m = Exchange.Utf8(market);
        Native.Order order;
        fixed (byte* mp = m)
        {
            Exchange.Check(Native.wickra_derivatives_close_position(_handle, mp, &order));
        }
        return Exchange.ReadOrder(order);
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            Native.wickra_derivatives_free(_handle);
            _handle = 0;
        }
    }
}

/// <summary>
/// One order in a batch placement. A <c>null</c> <see cref="Price"/> places a
/// market order; a value places a limit order.
/// </summary>
public sealed record BatchOrderRequest(string Market, Side Side, double Quantity, double? Price);

/// <summary>
/// One order's outcome in a batch placement: exactly one of <see cref="Order"/>
/// (success) or <see cref="Error"/> (per-order rejection) is set.
/// </summary>
public sealed record BatchResult(OrderInfo? Order, string? Error);

/// <summary>
/// A live advanced-orders client: amend, batch place/cancel and OCO. Construct
/// with <see cref="Connect"/>. Available on the eight trading venues.
/// </summary>
public sealed unsafe class AdvancedOrders : IDisposable
{
    private nint _handle;

    private AdvancedOrders(nint handle) => _handle = handle;

    /// <summary>Connect an advanced-orders client; <paramref name="futures"/> selects USDⓈ-M futures.</summary>
    public static AdvancedOrders Connect(
        string name, string apiKey, string apiSecret,
        string? passphrase = null, string? privateKey = null, bool testnet = false, bool futures = false)
    {
        nint pass = passphrase is null ? 0 : Marshal.StringToCoTaskMemUTF8(passphrase);
        nint priv = privateKey is null ? 0 : Marshal.StringToCoTaskMemUTF8(privateKey);
        var nameBytes = Exchange.Utf8(name);
        var keyBytes = Exchange.Utf8(apiKey);
        var secretBytes = Exchange.Utf8(apiSecret);
        try
        {
            fixed (byte* np = nameBytes)
            fixed (byte* kp = keyBytes)
            fixed (byte* sp = secretBytes)
            {
                nint handle = Native.wickra_connect_advanced(np, kp, sp, (byte*)pass, (byte*)priv, testnet, futures);
                if (handle == 0)
                {
                    throw new WickraException($"failed to connect advanced-orders client for {name}");
                }
                return new AdvancedOrders(handle);
            }
        }
        finally
        {
            if (pass != 0) { Marshal.FreeCoTaskMem(pass); }
            if (priv != 0) { Marshal.FreeCoTaskMem(priv); }
        }
    }

    /// <summary>Amend a resting order in place; pass <c>double.NaN</c> to leave a field unchanged.</summary>
    public OrderInfo AmendOrder(string market, string orderId, double newPrice, double newQuantity)
    {
        var m = Exchange.Utf8(market);
        var o = Exchange.Utf8(orderId);
        Native.Order order;
        fixed (byte* mp = m)
        fixed (byte* op = o)
        {
            Exchange.Check(Native.wickra_advanced_amend_order(_handle, mp, op, newPrice, newQuantity, &order));
        }
        return Exchange.ReadOrder(order);
    }

    /// <summary>Cancel several orders on <paramref name="market"/> in one request.</summary>
    public void CancelBatch(string market, IReadOnlyList<string> orderIds)
    {
        var m = Exchange.Utf8(market);
        var ids = new nint[orderIds.Count];
        for (int i = 0; i < orderIds.Count; i++)
        {
            ids[i] = Marshal.StringToCoTaskMemUTF8(orderIds[i]);
        }
        try
        {
            fixed (byte* mp = m)
            fixed (nint* ip = ids)
            {
                Exchange.Check(Native.wickra_advanced_cancel_batch(_handle, mp, ip, (nuint)ids.Length));
            }
        }
        finally
        {
            foreach (var ptr in ids)
            {
                if (ptr != 0) { Marshal.FreeCoTaskMem(ptr); }
            }
        }
    }

    /// <summary>
    /// Place a one-cancels-other bracket: a take-profit limit leg at
    /// <paramref name="price"/> paired with a stop leg triggered at
    /// <paramref name="stopPrice"/>. A non-null <paramref name="stopLimitPrice"/>
    /// makes the stop leg a stop-limit; <c>null</c> leaves it a stop-market.
    /// Returns the resulting order legs.
    /// </summary>
    public IReadOnlyList<OrderInfo> PlaceOco(
        string market, Side side, double quantity, double price, double stopPrice, double? stopLimitPrice = null)
    {
        var m = Exchange.Utf8(market);
        double slp = stopLimitPrice ?? double.NaN;
        int cap = 4;
        while (true)
        {
            var buffer = new Native.Order[cap];
            int count;
            fixed (byte* mp = m)
            fixed (Native.Order* bp = buffer)
            {
                count = Native.wickra_advanced_place_oco(
                    _handle, mp, (int)side, quantity, price, stopPrice, slp, bp, (nuint)cap);
            }
            if (count < 0)
            {
                throw new WickraException($"place_oco failed with code {count}");
            }
            if (count > cap)
            {
                cap = count;
                continue;
            }
            var result = new List<OrderInfo>(count);
            for (int i = 0; i < count; i++)
            {
                result.Add(Exchange.ReadOrder(buffer[i]));
            }
            return result;
        }
    }

    /// <summary>
    /// Place several orders in one request. Returns one <see cref="BatchResult"/>
    /// per request, in order: a per-order rejection surfaces in that result's
    /// <see cref="BatchResult.Error"/>, while a whole-request failure throws.
    /// </summary>
    public IReadOnlyList<BatchResult> PlaceBatch(IReadOnlyList<BatchOrderRequest> requests)
    {
        int n = requests.Count;
        if (n == 0)
        {
            return Array.Empty<BatchResult>();
        }
        var markets = new nint[n];
        var sides = new int[n];
        var quantities = new double[n];
        var prices = new double[n];
        for (int i = 0; i < n; i++)
        {
            markets[i] = Marshal.StringToCoTaskMemUTF8(requests[i].Market);
            sides[i] = (int)requests[i].Side;
            quantities[i] = requests[i].Quantity;
            prices[i] = requests[i].Price ?? double.NaN;
        }
        try
        {
            var outBuf = new Native.Order[n];
            var codes = new int[n];
            int count;
            fixed (nint* mp = markets)
            fixed (int* sp = sides)
            fixed (double* qp = quantities)
            fixed (double* pp = prices)
            fixed (Native.Order* op = outBuf)
            fixed (int* cp = codes)
            {
                count = Native.wickra_advanced_place_batch(
                    _handle, mp, sp, qp, pp, (nuint)n, op, cp, (nuint)n);
            }
            if (count < 0)
            {
                throw new WickraException($"place_batch failed with code {count}");
            }
            var result = new List<BatchResult>(count);
            for (int i = 0; i < count; i++)
            {
                result.Add(codes[i] == Native.Ok
                    ? new BatchResult(Exchange.ReadOrder(outBuf[i]), null)
                    : new BatchResult(null, $"order rejected with code {codes[i]}"));
            }
            return result;
        }
        finally
        {
            foreach (var ptr in markets)
            {
                if (ptr != 0) { Marshal.FreeCoTaskMem(ptr); }
            }
        }
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            Native.wickra_advanced_free(_handle);
            _handle = 0;
        }
    }
}

/// <summary>
/// A live private user-data client. After <see cref="Subscribe"/>, <see cref="Poll"/>
/// surfaces the account's own order and balance updates alongside the public
/// market-data stream. Available on the eight trading venues.
/// </summary>
public sealed unsafe class UserData : IDisposable
{
    private nint _handle;

    private UserData(nint handle) => _handle = handle;

    /// <summary>Connect a user-data client; <paramref name="futures"/> selects USDⓈ-M futures.</summary>
    public static UserData Connect(
        string name, string apiKey, string apiSecret,
        string? passphrase = null, string? privateKey = null, bool testnet = false, bool futures = false)
    {
        nint pass = passphrase is null ? 0 : Marshal.StringToCoTaskMemUTF8(passphrase);
        nint priv = privateKey is null ? 0 : Marshal.StringToCoTaskMemUTF8(privateKey);
        var nameBytes = Exchange.Utf8(name);
        var keyBytes = Exchange.Utf8(apiKey);
        var secretBytes = Exchange.Utf8(apiSecret);
        try
        {
            fixed (byte* np = nameBytes)
            fixed (byte* kp = keyBytes)
            fixed (byte* sp = secretBytes)
            {
                nint handle = Native.wickra_connect_user_data(np, kp, sp, (byte*)pass, (byte*)priv, testnet, futures);
                if (handle == 0)
                {
                    throw new WickraException($"failed to connect user-data client for {name}");
                }
                return new UserData(handle);
            }
        }
        finally
        {
            if (pass != 0) { Marshal.FreeCoTaskMem(pass); }
            if (priv != 0) { Marshal.FreeCoTaskMem(priv); }
        }
    }

    /// <summary>Open the private user-data stream; afterwards <see cref="Poll"/> drains the account's events too.</summary>
    public void Subscribe()
    {
        Exchange.Check(Native.wickra_user_data_subscribe(_handle));
    }

    /// <summary>Drain buffered events (up to <paramref name="capacity"/> per call).</summary>
    public IReadOnlyList<EventInfo> Poll(int capacity = 16)
    {
        var buffer = new Native.Event[capacity];
        int count;
        fixed (Native.Event* bp = buffer)
        {
            count = Native.wickra_user_data_poll(_handle, bp, (nuint)capacity);
        }
        if (count < 0)
        {
            throw new WickraException($"user-data poll failed with code {count}");
        }
        var events = new List<EventInfo>(count);
        for (int i = 0; i < count; i++)
        {
            events.Add(Exchange.ReadEvent(buffer[i]));
        }
        return events;
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            Native.wickra_user_data_free(_handle);
            _handle = 0;
        }
    }
}

/// <summary>
/// A live WebSocket order-API client: place and cancel orders over the venue's
/// WebSocket order API. Native on Binance/Bybit/OKX/Gate/Kraken; on Bitget,
/// KuCoin and HTX the methods throw (no WebSocket order-entry API — use REST).
/// </summary>
public sealed unsafe class WsExecution : IDisposable
{
    private nint _handle;

    private WsExecution(nint handle) => _handle = handle;

    /// <summary>Connect a WebSocket order-API client; <paramref name="futures"/> selects USDⓈ-M futures.</summary>
    public static WsExecution Connect(
        string name, string apiKey, string apiSecret,
        string? passphrase = null, string? privateKey = null, bool testnet = false, bool futures = false)
    {
        nint pass = passphrase is null ? 0 : Marshal.StringToCoTaskMemUTF8(passphrase);
        nint priv = privateKey is null ? 0 : Marshal.StringToCoTaskMemUTF8(privateKey);
        var nameBytes = Exchange.Utf8(name);
        var keyBytes = Exchange.Utf8(apiKey);
        var secretBytes = Exchange.Utf8(apiSecret);
        try
        {
            fixed (byte* np = nameBytes)
            fixed (byte* kp = keyBytes)
            fixed (byte* sp = secretBytes)
            {
                nint handle = Native.wickra_connect_ws_execution(np, kp, sp, (byte*)pass, (byte*)priv, testnet, futures);
                if (handle == 0)
                {
                    throw new WickraException($"failed to connect ws-execution client for {name}");
                }
                return new WsExecution(handle);
            }
        }
        finally
        {
            if (pass != 0) { Marshal.FreeCoTaskMem(pass); }
            if (priv != 0) { Marshal.FreeCoTaskMem(priv); }
        }
    }

    /// <summary>Place an order over the WebSocket order API. <c>double.NaN</c> price = market order.</summary>
    public OrderInfo PlaceOrderWs(string market, Side side, double quantity, double price)
    {
        var m = Exchange.Utf8(market);
        Native.Order order;
        fixed (byte* mp = m)
        {
            Exchange.Check(Native.wickra_ws_place_order(_handle, mp, (int)side, quantity, price, &order));
        }
        return Exchange.ReadOrder(order);
    }

    /// <summary>Cancel an order over the WebSocket order API by venue id.</summary>
    public void CancelOrderWs(string market, string orderId)
    {
        var m = Exchange.Utf8(market);
        var o = Exchange.Utf8(orderId);
        fixed (byte* mp = m)
        fixed (byte* op = o)
        {
            Exchange.Check(Native.wickra_ws_cancel_order(_handle, mp, op));
        }
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            Native.wickra_ws_execution_free(_handle);
            _handle = 0;
        }
    }
}
