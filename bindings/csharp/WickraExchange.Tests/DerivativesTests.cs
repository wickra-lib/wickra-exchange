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
}
