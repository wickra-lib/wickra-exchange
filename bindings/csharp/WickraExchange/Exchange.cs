using System.Runtime.InteropServices;
using System.Text;

namespace WickraExchange;

/// <summary>An error raised by the exchange layer.</summary>
public sealed class WickraException : Exception
{
    public WickraException(string message) : base(message) { }
}

/// <summary>The side of an order.</summary>
public enum Side
{
    Buy = Native.SideBuy,
    Sell = Native.SideSell,
}

/// <summary>The lifecycle state of an order.</summary>
public enum OrderStatus
{
    New = Native.StatusNew,
    PartiallyFilled = Native.StatusPartiallyFilled,
    Filled = Native.StatusFilled,
    Canceled = Native.StatusCanceled,
    Rejected = Native.StatusRejected,
    Expired = Native.StatusExpired,
}

/// <summary>The kind of a stream event.</summary>
public enum EventKind
{
    Trade = Native.EventTrade,
    Ticker = Native.EventTicker,
    OrderUpdate = Native.EventOrderUpdate,
    BalanceUpdate = Native.EventBalanceUpdate,
    Subscribed = Native.EventSubscribed,
    Other = Native.EventOther,
}

/// <summary>An order as reported by the exchange.</summary>
public sealed record OrderInfo(string Id, Side Side, OrderStatus Status, double Quantity, double FilledQuantity, double? Price, double? AveragePrice)
{
    /// <summary>Whether the order is fully filled.</summary>
    public bool IsFilled => Status == OrderStatus.Filled;
}

/// <summary>A single stream event.</summary>
public sealed record EventInfo(EventKind Kind, string? Symbol, double? Price, double? Quantity, Side? Side, OrderInfo? Order)
{
    public bool IsTrade => Kind == EventKind.Trade;
}

/// <summary>
/// A unified exchange client over the synchronous, pull-based API. Construct with
/// <see cref="Paper"/>, <see cref="ReplayTrades"/> or <see cref="Connect"/>; the
/// methods are identical whichever backend was chosen.
/// </summary>
public sealed unsafe class Exchange : IDisposable
{
    private nint _handle;

    private Exchange(nint handle) => _handle = handle;

    /// <summary>The library version.</summary>
    public static string Version()
    {
        return Marshal.PtrToStringUTF8(Native.wickra_version()) ?? string.Empty;
    }

    /// <summary>An offline paper account seeded from <paramref name="balances"/>.</summary>
    public static Exchange Paper(
        IReadOnlyDictionary<string, double> balances,
        double makerBps = 0.0, double takerBps = 0.0, double slippageBps = 0.0)
    {
        var (assets, amounts) = MarshalBalances(balances);
        try
        {
            fixed (nint* ap = assets)
            fixed (double* am = amounts)
            {
                nint handle = Native.wickra_paper_new(ap, am, (nuint)balances.Count, makerBps, takerBps, slippageBps);
                if (handle == 0)
                {
                    throw new WickraException("failed to construct paper exchange");
                }
                return new Exchange(handle);
            }
        }
        finally
        {
            FreeMarshalled(assets);
        }
    }

    /// <summary>A replay account driven by a recorded <paramref name="tape"/> of trades.</summary>
    public static Exchange ReplayTrades(
        string market, IReadOnlyList<double> tape, IReadOnlyDictionary<string, double> balances,
        double makerBps = 0.0, double takerBps = 0.0, double slippageBps = 0.0)
    {
        var (assets, amounts) = MarshalBalances(balances);
        var marketBytes = Utf8(market);
        var tapeArray = tape.ToArray();
        try
        {
            fixed (byte* mp = marketBytes)
            fixed (double* tp = tapeArray)
            fixed (nint* ap = assets)
            fixed (double* am = amounts)
            {
                nint handle = Native.wickra_replay_new(
                    mp, tp, (nuint)tapeArray.Length, ap, am, (nuint)balances.Count,
                    makerBps, takerBps, slippageBps);
                if (handle == 0)
                {
                    throw new WickraException("failed to construct replay exchange");
                }
                return new Exchange(handle);
            }
        }
        finally
        {
            FreeMarshalled(assets);
        }
    }

    /// <summary>A live client for <paramref name="name"/> authenticated with API keys.</summary>
    public static Exchange Connect(
        string name, string apiKey, string apiSecret,
        string? passphrase = null, string? privateKey = null, bool testnet = false)
    {
        nint pass = passphrase is null ? 0 : Marshal.StringToCoTaskMemUTF8(passphrase);
        nint priv = privateKey is null ? 0 : Marshal.StringToCoTaskMemUTF8(privateKey);
        var nameBytes = Utf8(name);
        var keyBytes = Utf8(apiKey);
        var secretBytes = Utf8(apiSecret);
        try
        {
            fixed (byte* np = nameBytes)
            fixed (byte* kp = keyBytes)
            fixed (byte* sp = secretBytes)
            {
                nint handle = Native.wickra_connect(np, kp, sp, (byte*)pass, (byte*)priv, testnet);
                if (handle == 0)
                {
                    throw new WickraException($"failed to connect to {name}");
                }
                return new Exchange(handle);
            }
        }
        finally
        {
            if (pass != 0) { Marshal.FreeCoTaskMem(pass); }
            if (priv != 0) { Marshal.FreeCoTaskMem(priv); }
        }
    }

    /// <summary>The venue identifier (<c>"paper"</c>, <c>"replay"</c>, <c>"binance"</c>, ...).</summary>
    public string Name()
    {
        Span<byte> buf = stackalloc byte[32];
        fixed (byte* bp = buf)
        {
            Check(Native.wickra_exchange_name(_handle, bp, (nuint)buf.Length));
        }
        return CString(buf);
    }

    /// <summary>Set the mark price a paper account fills against (paper backend only).</summary>
    public void SetPrice(string market, double price)
    {
        var m = Utf8(market);
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_set_price(_handle, mp, price));
        }
    }

    /// <summary>Place a market order.</summary>
    public OrderInfo PlaceMarket(string market, Side side, double quantity)
    {
        var m = Utf8(market);
        Native.Order order;
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_place_market(_handle, mp, (int)side, quantity, &order));
        }
        return ReadOrder(order);
    }

    /// <summary>Place a limit order.</summary>
    public OrderInfo PlaceLimit(string market, Side side, double quantity, double price)
    {
        var m = Utf8(market);
        Native.Order order;
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_place_limit(_handle, mp, (int)side, quantity, price, &order));
        }
        return ReadOrder(order);
    }

    /// <summary>Cancel an open order by venue id.</summary>
    public void Cancel(string market, string orderId)
    {
        var m = Utf8(market);
        var o = Utf8(orderId);
        fixed (byte* mp = m)
        fixed (byte* op = o)
        {
            Check(Native.wickra_exchange_cancel(_handle, mp, op));
        }
    }

    /// <summary>The free balance of <paramref name="asset"/>.</summary>
    public double Balance(string asset)
    {
        var a = Utf8(asset);
        double free;
        fixed (byte* ap = a)
        {
            Check(Native.wickra_exchange_balance(_handle, ap, &free));
        }
        return free;
    }

    /// <summary>Drain buffered events (up to <paramref name="capacity"/> per call).</summary>
    public IReadOnlyList<EventInfo> Poll(int capacity = 16)
    {
        var buffer = new Native.Event[capacity];
        int count;
        fixed (Native.Event* bp = buffer)
        {
            count = Native.wickra_exchange_poll(_handle, bp, (nuint)capacity);
        }
        if (count < 0)
        {
            throw new WickraException($"poll failed with code {count}");
        }
        var events = new List<EventInfo>(count);
        for (int i = 0; i < count; i++)
        {
            events.Add(ReadEvent(buffer[i]));
        }
        return events;
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            Native.wickra_exchange_free(_handle);
            _handle = 0;
        }
    }

    // ---------------------------- helpers ------------------------------------

    internal static byte[] Utf8(string value)
    {
        int len = Encoding.UTF8.GetByteCount(value);
        var bytes = new byte[len + 1];
        Encoding.UTF8.GetBytes(value, bytes);
        bytes[len] = 0;
        return bytes;
    }

    private static (nint[] assets, double[] amounts) MarshalBalances(IReadOnlyDictionary<string, double> balances)
    {
        var assets = new nint[balances.Count];
        var amounts = new double[balances.Count];
        int i = 0;
        foreach (var kv in balances)
        {
            assets[i] = Marshal.StringToCoTaskMemUTF8(kv.Key);
            amounts[i] = kv.Value;
            i++;
        }
        return (assets, amounts);
    }

    private static void FreeMarshalled(nint[] assets)
    {
        foreach (var ptr in assets)
        {
            if (ptr != 0) { Marshal.FreeCoTaskMem(ptr); }
        }
    }

    internal static string CString(ReadOnlySpan<byte> buf)
    {
        int end = buf.IndexOf((byte)0);
        return Encoding.UTF8.GetString(end < 0 ? buf : buf[..end]);
    }

    internal static PositionInfo ReadPosition(Native.Position pos)
    {
        var symbol = CString(new Span<byte>(pos.Symbol, Native.StrCap));
        return new PositionInfo(
            symbol, (PositionSide)pos.Side, pos.Quantity, pos.EntryPrice,
            pos.MarkPrice, pos.Leverage, pos.UnrealizedPnl, (MarginMode)pos.MarginMode);
    }

    internal static OrderInfo ReadOrder(Native.Order order)
    {
        string id;
        var span = new Span<byte>(order.Id, Native.StrCap);
        id = CString(span);
        double? price = double.IsNaN(order.Price) ? null : order.Price;
        double? avg = double.IsNaN(order.AveragePrice) ? null : order.AveragePrice;
        return new OrderInfo(id, (Side)order.Side, (OrderStatus)order.Status, order.Quantity, order.FilledQuantity, price, avg);
    }

    internal static EventInfo ReadEvent(Native.Event ev)
    {
        string? symbol = null;
        var span = new Span<byte>(ev.Symbol, Native.StrCap);
        var s = CString(span);
        if (s.Length > 0) { symbol = s; }
        double? price = double.IsNaN(ev.Price) ? null : ev.Price;
        double? qty = double.IsNaN(ev.Quantity) ? null : ev.Quantity;
        Side? side = ev.Side < 0 ? null : (Side)ev.Side;
        OrderInfo? order = ev.Kind == Native.EventOrderUpdate ? ReadOrder(ev.Order) : null;
        return new EventInfo((EventKind)ev.Kind, symbol, price, qty, side, order);
    }

    internal static void Check(int code)
    {
        if (code != Native.Ok)
        {
            throw new WickraException($"exchange call failed with code {code}");
        }
    }
}
