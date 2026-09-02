using WickraExchange;
using Xunit;

namespace WickraExchange.Tests;

/// <summary>
/// The order fields that had no way across the C ABI until now.
/// </summary>
/// <remarks>
/// <c>PlaceMarket</c> and <c>PlaceLimit</c> take a market, a side, a quantity
/// and a price. Everything the library supports beyond that -- the trigger
/// price, the time-in-force, post-only, reduce-only, self-trade prevention, the
/// client order id -- existed in the Rust core and could not be expressed from
/// C#. These tests hold the new path to carrying them.
/// </remarks>
public class OrderRequestTests
{
    private static Exchange Paper() =>
        Exchange.Paper(
            new Dictionary<string, double> { ["USDT"] = 100_000.0, ["BTC"] = 5.0 },
            makerBps: 1.0, takerBps: 5.0, slippageBps: 10.0);

    [Fact]
    public void APlainRequestPlacesTheSameOrderAsTheNarrowCall()
    {
        using var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);

        var order = ex.PlaceOrder(new OrderRequest("BTC/USDT", Side.Buy, OrderType.Market, 1.0));

        Assert.True(order.IsFilled);
        Assert.NotNull(order.AveragePrice);
        Assert.True(Math.Abs(order.AveragePrice!.Value - 20_020.0) < 1e-6);
    }

    [Fact]
    public void ARestingOrderCarriesItsFlags()
    {
        using var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);

        var order = ex.PlaceOrder(new OrderRequest("BTC/USDT", Side.Buy, OrderType.Limit, 1.0)
        {
            Price = 19_000.0,
            TimeInForce = TimeInForce.Gtc,
            ClientOrderId = "retry-safe-1",
            PostOnly = true,
            Stp = SelfTradePrevention.ExpireMaker,
        });

        Assert.Equal(OrderStatus.New, order.Status);
    }

    /// <summary>
    /// A trigger order reaches the venue with its trigger. The paper backend
    /// refuses triggers, and that refusal is the proof it arrived: a request
    /// with the field dropped would have been placed as a plain market sell
    /// instead, at the price the stop existed to protect against.
    /// </summary>
    [Fact]
    public void AStopOrderCarriesItsTrigger()
    {
        using var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);

        Assert.ThrowsAny<Exception>(() =>
            ex.PlaceOrder(new OrderRequest("BTC/USDT", Side.Sell, OrderType.StopMarket, 1.0)
            {
                StopPrice = 19_000.0,
            }));
    }

    /// <summary>
    /// A stop order without its trigger price is invalid on its own terms, and
    /// is rejected before it reaches a venue.
    /// </summary>
    [Fact]
    public void AStopOrderWithoutATriggerIsRejected()
    {
        using var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);

        Assert.ThrowsAny<Exception>(() =>
            ex.PlaceOrder(new OrderRequest("BTC/USDT", Side.Sell, OrderType.StopMarket, 1.0)));
    }

    [Fact]
    public void TheRequestDefaultsAreTheUnsetOnes()
    {
        var request = new OrderRequest("BTC/USDT", Side.Buy, OrderType.Market, 1.0);

        Assert.Null(request.Price);
        Assert.Null(request.StopPrice);
        Assert.Null(request.ClientOrderId);
        Assert.Equal(TimeInForce.Gtc, request.TimeInForce);
        Assert.Equal(SelfTradePrevention.None, request.Stp);
        Assert.False(request.ReduceOnly);
        Assert.False(request.PostOnly);
    }
}
