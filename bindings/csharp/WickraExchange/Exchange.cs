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

/// <summary>What kind of order this is: whether it takes the market now, rests
/// at a price, or waits for a trigger.</summary>
public enum OrderType
{
    /// <summary>Takes the best available price now.</summary>
    Market = Native.OrderMarket,
    /// <summary>Rests at the limit price until filled or cancelled.</summary>
    Limit = Native.OrderLimit,
    /// <summary>Rests until the market reaches the stop price, then takes the market.</summary>
    StopMarket = Native.OrderStopMarket,
    /// <summary>Rests until the market reaches the stop price, then rests at the limit price.</summary>
    StopLimit = Native.OrderStopLimit,
}

/// <summary>How long an order may live.</summary>
public enum TimeInForce
{
    /// <summary>Rest until cancelled.</summary>
    Gtc = Native.TifGtc,
    /// <summary>Fill what is possible now, cancel the rest.</summary>
    Ioc = Native.TifIoc,
    /// <summary>Fill entirely now or not at all.</summary>
    Fok = Native.TifFok,
}

/// <summary>Which side to cancel when an order would match the account's own
/// resting order.</summary>
public enum SelfTradePrevention
{
    /// <summary>Let the account trade against itself.</summary>
    None = Native.StpNone,
    /// <summary>Cancel the resting order.</summary>
    ExpireMaker = Native.StpExpireMaker,
    /// <summary>Cancel the incoming order.</summary>
    ExpireTaker = Native.StpExpireTaker,
    /// <summary>Cancel both.</summary>
    ExpireBoth = Native.StpExpireBoth,
}

/// <summary>A full order, as the caller wants it placed.</summary>
/// <remarks>
/// <see cref="Exchange.PlaceMarket"/> and <see cref="Exchange.PlaceLimit"/> take
/// a market, a side, a quantity and a price, which is all an order could ever be
/// from this binding. Everything else the library supports had no way through:
/// the trigger price that makes a stop-loss a stop-loss, the time-in-force that
/// says an order must not rest, post-only, reduce-only, self-trade prevention,
/// and the client order id that makes a retried placement idempotent.
/// </remarks>
public sealed record OrderRequest(string Market, Side Side, OrderType Type, double Quantity)
{
    /// <summary>Limit price. Null for a market order.</summary>
    public double? Price { get; init; }

    /// <summary>Trigger price the order rests for. Null for a non-trigger order.</summary>
    public double? StopPrice { get; init; }

    /// <summary>How long the order may live. Defaults to good-til-cancelled.</summary>
    public TimeInForce TimeInForce { get; init; } = TimeInForce.Gtc;

    /// <summary>An id of the caller's choosing, so a retried placement is
    /// recognised by the venue as the same order rather than placed twice.</summary>
    public string? ClientOrderId { get; init; }

    /// <summary>Close-only: the order may not increase a position.</summary>
    public bool ReduceOnly { get; init; }

    /// <summary>Maker-only: the order is cancelled rather than crossing the spread.</summary>
    public bool PostOnly { get; init; }

    /// <summary>Self-trade-prevention policy.</summary>
    public SelfTradePrevention Stp { get; init; } = SelfTradePrevention.None;
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

/// <summary>A point-in-time ticker snapshot.</summary>
/// <summary>A point-in-time quote.</summary>
/// <param name="Timestamp">The venue own stamp in milliseconds since the Unix
/// epoch, or 0 when the venue published none -- never the local clock, since a
/// locally stamped quote looks fresh by construction.</param>
public sealed record TickerInfo(string Symbol, double Last, double Bid, double Ask, double Volume,
    long Timestamp);

/// <summary>A single OHLCV candle.</summary>
public sealed record CandleInfo(double Open, double High, double Low, double Close, double Volume, long Timestamp);

/// <summary>A single order-book level: price and resting quantity.</summary>
public sealed record BookLevelInfo(double Price, double Quantity);

/// <summary>
/// A depth snapshot, best-first on each side. <see cref="Symbol"/> echoes the
/// requested market; the venue sequence id is available on the native bindings.
/// </summary>
public sealed record OrderBookInfo(string Symbol, IReadOnlyList<BookLevelInfo> Bids, IReadOnlyList<BookLevelInfo> Asks);

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
        string? passphrase = null, string? privateKey = null, bool testnet = false,
        Market market = Market.Spot,
        MarginMode marginMode = MarginMode.Cross,
        PositionMode positionMode = PositionMode.OneWay)
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
                nint handle = Native.wickra_connect(
                    np, kp, sp, (byte*)pass, (byte*)priv, testnet,
                    (int)market, (int)marginMode, (int)positionMode);
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

    /// <summary>
    /// Place a full order: every field the library supports, not just a market,
    /// a side, a quantity and a price.
    /// </summary>
    /// <remarks>
    /// <see cref="PlaceMarket"/> and <see cref="PlaceLimit"/> remain as the
    /// shortest spelling of the common case. This is the one that can place a
    /// stop-loss, an immediate-or-cancel, a post-only or an idempotent retry.
    /// A field the venue cannot express refuses the order rather than weakening
    /// it, which arrives here as an exception rather than as a
    /// differently-shaped order reaching the exchange.
    /// </remarks>
    public OrderInfo PlaceOrder(OrderRequest request)
    {
        var m = Utf8(request.Market);
        var c = request.ClientOrderId is null ? null : Utf8(request.ClientOrderId);
        Native.Order order;
        fixed (byte* mp = m)
        fixed (byte* cp = c)
        {
            var native = new Native.OrderRequest
            {
                Market = mp,
                Side = (int)request.Side,
                OrderType = (int)request.Type,
                Quantity = request.Quantity,
                Price = request.Price ?? double.NaN,
                StopPrice = request.StopPrice ?? double.NaN,
                TimeInForce = (int)request.TimeInForce,
                ClientOrderId = cp,
                ReduceOnly = request.ReduceOnly,
                PostOnly = request.PostOnly,
                Stp = (int)request.Stp,
            };
            Check(Native.wickra_exchange_place_order(_handle, &native, &order));
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

    /// <summary>The current ticker for <paramref name="market"/>.</summary>
    public TickerInfo Ticker(string market)
    {
        var m = Utf8(market);
        Native.Ticker t;
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_ticker(_handle, mp, &t));
        }
        return ReadTicker(t);
    }

    /// <summary>Up to <paramref name="limit"/> historical candles for <paramref name="market"/> at <paramref name="interval"/>.</summary>
    public IReadOnlyList<CandleInfo> Klines(string market, string interval, uint limit)
    {
        var m = Utf8(market);
        var iv = Utf8(interval);
        int cap = 128;
        while (true)
        {
            var buffer = new Native.Candle[cap];
            int count;
            fixed (byte* mp = m)
            fixed (byte* ip = iv)
            fixed (Native.Candle* bp = buffer)
            {
                count = Native.wickra_exchange_klines(_handle, mp, ip, limit, bp, (nuint)cap);
            }
            if (count < 0)
            {
                throw new WickraException($"klines failed with code {count}");
            }
            if (count > cap)
            {
                cap = count;
                continue;
            }
            var result = new List<CandleInfo>(count);
            for (int i = 0; i < count; i++)
            {
                var c = buffer[i];
                result.Add(new CandleInfo(c.Open, c.High, c.Low, c.Close, c.Volume, c.Timestamp));
            }
            return result;
        }
    }

    /// <summary>Depth snapshot for <paramref name="market"/> (up to <paramref name="depth"/> levels per side).</summary>
    public OrderBookInfo OrderBook(string market, uint depth)
    {
        var m = Utf8(market);
        int cap = 64;
        while (true)
        {
            var bids = new Native.BookLevel[cap];
            var asks = new Native.BookLevel[cap];
            nuint bidCount, askCount;
            int rc;
            fixed (byte* mp = m)
            fixed (Native.BookLevel* bp = bids)
            fixed (Native.BookLevel* ap = asks)
            {
                rc = Native.wickra_exchange_order_book(
                    _handle, mp, depth, bp, (nuint)cap, ap, (nuint)cap, &bidCount, &askCount);
            }
            Check(rc);
            int nb = (int)bidCount, na = (int)askCount;
            if (nb > cap || na > cap)
            {
                cap = Math.Max(nb, na);
                continue;
            }
            var bidList = new List<BookLevelInfo>(nb);
            for (int i = 0; i < nb; i++)
            {
                bidList.Add(new BookLevelInfo(bids[i].Price, bids[i].Quantity));
            }
            var askList = new List<BookLevelInfo>(na);
            for (int i = 0; i < na; i++)
            {
                askList.Add(new BookLevelInfo(asks[i].Price, asks[i].Quantity));
            }
            return new OrderBookInfo(market, bidList, askList);
        }
    }

    /// <summary>Subscribe to the public trade stream for <paramref name="market"/>.</summary>
    public void SubscribeTrades(string market)
    {
        var m = Utf8(market);
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_subscribe_trades(_handle, mp));
        }
    }

    /// <summary>Subscribe to the order-book stream for <paramref name="market"/>.</summary>
    public void SubscribeBook(string market)
    {
        var m = Utf8(market);
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_subscribe_book(_handle, mp));
        }
    }

    /// <summary>Subscribe to the ticker stream for <paramref name="market"/>.</summary>
    public void SubscribeTicker(string market)
    {
        var m = Utf8(market);
        fixed (byte* mp = m)
        {
            Check(Native.wickra_exchange_subscribe_ticker(_handle, mp));
        }
    }

    /// <summary>Look up a single order by venue id.</summary>
    public OrderInfo QueryOrder(string market, string orderId)
    {
        var m = Utf8(market);
        var o = Utf8(orderId);
        Native.Order order;
        fixed (byte* mp = m)
        fixed (byte* op = o)
        {
            Check(Native.wickra_exchange_query_order(_handle, mp, op, &order));
        }
        return ReadOrder(order);
    }

    /// <summary>Open orders, optionally filtered to one <paramref name="market"/>.</summary>
    public IReadOnlyList<OrderInfo> OpenOrders(string? market = null)
    {
        byte[]? m = market is null ? null : Utf8(market);
        int cap = 16;
        while (true)
        {
            var buffer = new Native.Order[cap];
            int count;
            fixed (byte* mp = m)
            fixed (Native.Order* bp = buffer)
            {
                count = Native.wickra_exchange_open_orders(_handle, mp, bp, (nuint)cap);
            }
            if (count < 0)
            {
                throw new WickraException($"open_orders failed with code {count}");
            }
            if (count > cap)
            {
                cap = count;
                continue;
            }
            var result = new List<OrderInfo>(count);
            for (int i = 0; i < count; i++)
            {
                result.Add(ReadOrder(buffer[i]));
            }
            return result;
        }
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

    /// <summary>
    /// Project a managed <see cref="OrderRequest"/> onto the C-ABI struct, with
    /// its two strings in unmanaged memory.
    /// </summary>
    /// <remarks>
    /// The caller owns the returned pointers and must free them with
    /// <see cref="FreeNative"/>; unmanaged rather than pinned, because the batch
    /// path needs an array of these alive at once and <c>fixed</c> pins one at a
    /// time. Every path that sends an order goes through here, so the fields a
    /// batch or a WebSocket frame carries cannot drift from the fields a single
    /// order carries.
    /// </remarks>
    internal static Native.OrderRequest ToNative(OrderRequest request)
    {
        return new Native.OrderRequest
        {
            Market = (byte*)Marshal.StringToCoTaskMemUTF8(request.Market),
            Side = (int)request.Side,
            OrderType = (int)request.Type,
            Quantity = request.Quantity,
            Price = request.Price ?? double.NaN,
            StopPrice = request.StopPrice ?? double.NaN,
            TimeInForce = (int)request.TimeInForce,
            ClientOrderId = request.ClientOrderId is null
                ? null
                : (byte*)Marshal.StringToCoTaskMemUTF8(request.ClientOrderId),
            ReduceOnly = request.ReduceOnly,
            PostOnly = request.PostOnly,
            Stp = (int)request.Stp,
        };
    }

    /// <summary>Free the strings <see cref="ToNative"/> allocated.</summary>
    internal static void FreeNative(Native.OrderRequest native)
    {
        if (native.Market is not null) { Marshal.FreeCoTaskMem((nint)native.Market); }
        if (native.ClientOrderId is not null) { Marshal.FreeCoTaskMem((nint)native.ClientOrderId); }
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

    internal static TickerInfo ReadTicker(Native.Ticker t)
    {
        var symbol = CString(new Span<byte>(t.Symbol, Native.StrCap));
        return new TickerInfo(symbol, t.Last, t.Bid, t.Ask, t.Volume, t.Timestamp);
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
