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
        var symbol = Exchange.CString(new Span<byte>(pos.Symbol, Native.StrCap));
        return new PositionInfo(
            symbol, (PositionSide)pos.Side, pos.Quantity, pos.EntryPrice,
            pos.MarkPrice, pos.Leverage, pos.UnrealizedPnl, (MarginMode)pos.MarginMode);
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
/// A live advanced-orders client: amend and batch cancel. Construct with
/// <see cref="Connect"/>. Available on the eight trading venues.
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

    public void Dispose()
    {
        if (_handle != 0)
        {
            Native.wickra_advanced_free(_handle);
            _handle = 0;
        }
    }
}
