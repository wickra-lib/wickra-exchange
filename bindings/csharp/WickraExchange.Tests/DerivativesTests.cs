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
}
