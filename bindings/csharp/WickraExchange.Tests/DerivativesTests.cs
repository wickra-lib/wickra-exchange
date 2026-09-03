using WickraExchange;
using Xunit;

namespace WickraExchange.Tests;

// Construction is offline (no socket opens until an RPC is issued), so the
// surface and the spot-only rejection are checked without a network.
public class DerivativesTests
{
    [Theory]
    [InlineData("coinbase")]
    [InlineData("upbit")]
    [InlineData("ftx")]
    public void DerivativesRejectsSpotOnlyAndUnknown(string name)
    {
        Assert.Throws<WickraException>(() => Derivatives.Connect(name, "k", "s"));
    }

    [Theory]
    [InlineData("coinbase")]
    [InlineData("upbit")]
    [InlineData("ftx")]
    public void AdvancedRejectsSpotOnlyAndUnknown(string name)
    {
        Assert.Throws<WickraException>(() => AdvancedOrders.Connect(name, "k", "s"));
    }

    [Fact]
    public void DerivativesAndAdvancedConstructForFuturesVenue()
    {
        using var d = Derivatives.Connect("binance", "k", "s");
        using var a = AdvancedOrders.Connect("binance", "k", "s", futures: true);
        Assert.NotNull(d);
        Assert.NotNull(a);
    }

    [Fact]
    public void PlaceBatchEmptyIsNoop()
    {
        // An empty batch returns without opening a socket.
        using var a = AdvancedOrders.Connect("binance", "k", "s");
        var results = a.PlaceBatch(System.Array.Empty<BatchOrderRequest>());
        Assert.Empty(results);
    }

    [Fact]
    public void PlaceBatchFullEmptyIsNoop()
    {
        // The full-request batch, which is the one that can carry a stop-loss.
        // Empty returns without opening a socket, like its narrow sibling.
        using var a = AdvancedOrders.Connect("binance", "k", "s");
        var results = a.PlaceBatch(System.Array.Empty<OrderRequest>());
        Assert.Empty(results);
    }

    [Fact]
    public void ABatchedOrderCanCarryEveryField()
    {
        // The shape the narrow BatchOrderRequest has no room for. Nothing is
        // sent here -- what this pins is that the binding can express it at all,
        // which it could not: a batched order from C# was a market or a limit
        // and nothing else, whatever the venue clients supported.
        var request = new OrderRequest("BTC/USDT", Side.Buy, OrderType.StopLimit, 1.0)
        {
            Price = 100.0,
            StopPrice = 95.0,
            TimeInForce = TimeInForce.Ioc,
            ClientOrderId = "batch-1",
            ReduceOnly = true,
            PostOnly = true,
            Stp = SelfTradePrevention.ExpireMaker,
        };
        Assert.Equal(95.0, request.StopPrice);
        Assert.Equal(TimeInForce.Ioc, request.TimeInForce);
        Assert.Equal("batch-1", request.ClientOrderId);
    }

    [Fact]
    public void BatchRequestShapeRoundTrips()
    {
        var requests = new[]
        {
            new BatchOrderRequest("BTC/USDT", Side.Buy, 0.5, 60000),
            new BatchOrderRequest("ETH/USDT", Side.Sell, 2, null),
        };
        Assert.Equal(2, requests.Length);
        Assert.Equal(Side.Buy, requests[0].Side);
        Assert.Null(requests[1].Price);
    }

    [Theory]
    [InlineData("coinbase")]
    [InlineData("upbit")]
    [InlineData("ftx")]
    public void UserDataAndWsExecutionRejectSpotOnlyAndUnknown(string name)
    {
        Assert.Throws<WickraException>(() => UserData.Connect(name, "k", "s"));
        Assert.Throws<WickraException>(() => WsExecution.Connect(name, "k", "s"));
    }

    [Fact]
    public void UserDataConstructsAndPolls()
    {
        using var userData = UserData.Connect("binance", "k", "s");
        Assert.NotNull(userData);
        // Keepalive is a no-op before Subscribe; it must not throw.
        userData.Keepalive();
        // WsUserData: MarketData, so the client can poll (nothing buffered offline).
        Assert.Empty(userData.Poll());
    }

    [Fact]
    public void WsExecutionConstructsForATradingVenue()
    {
        using var exec = WsExecution.Connect("bybit", "k", "s");
        Assert.NotNull(exec);
    }
}
