using System.Globalization;
using WickraExchange;
using Xunit;

namespace WickraExchange.Tests;

/// <summary>
/// An order number arrives as the number that was written.
/// </summary>
/// <remarks>
/// A <c>double</c> holds about fifteen significant digits; the core holds every
/// order number in an exact decimal, and C#'s own <c>decimal</c> holds
/// twenty-eight. Sent as a double, <c>12345678.90123456789</c> arrives as
/// <c>12345678.90123457</c> — a different order, placed without a word. The
/// exact properties are the way a C# decimal reaches the venue as written.
///
/// What these can observe is that the exact value is the one that is used:
/// every number this binding reports is a double, so the last digits of a wide
/// number cannot be read back through it. That the wide number itself survives
/// the crossing is held by the C ABI's own tests, where it can be read exactly.
/// </remarks>
public class ExactNumberTests
{
    private const string Wide = "12345678.90123456789";

    private static Exchange Paper() =>
        Exchange.Paper(
            new Dictionary<string, double> { ["USDT"] = 1_000_000.0, ["BTC"] = 100.0 });

    [Fact]
    public void ADoubleCannotHoldTheNumberADecimalCan()
    {
        // The premise, measured rather than asserted: this is why the exact
        // properties exist.
        var exact = decimal.Parse(Wide, CultureInfo.InvariantCulture);
        var throughDouble = (decimal)(double)exact;
        Assert.NotEqual(exact, throughDouble);
    }

    [Fact]
    public void AnExactQuantityIsTheOneThatIsUsed()
    {
        var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);
        // The double says one thing and the exact value another, far enough
        // apart to be read back through a double: the exact one is the order.
        var request = new OrderRequest("BTC/USDT", Side.Sell, OrderType.Limit, 999.0)
        {
            Price = 21_000.0,
            ExactQuantity = 1.5m,
        };
        var order = ex.PlaceOrder(request);
        Assert.Equal(1.5, order.Quantity, 9);
    }

    [Fact]
    public void AnExactPriceIsTheOneThatIsUsed()
    {
        var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);
        var request = new OrderRequest("BTC/USDT", Side.Sell, OrderType.Limit, 1.0)
        {
            Price = 21_000.0,
            ExactPrice = 21_111.25m,
        };
        var order = ex.PlaceOrder(request);
        Assert.Equal(OrderStatus.New, order.Status);
        Assert.Equal(21_111.25, order.Price!.Value, 9);
    }

    [Fact]
    public void WithoutTheExactPropertiesNothingChanges()
    {
        var ex = Paper();
        ex.SetPrice("BTC/USDT", 20_000.0);
        var request = new OrderRequest("BTC/USDT", Side.Buy, OrderType.Limit, 1.0)
        {
            Price = 19_000.0,
        };
        var order = ex.PlaceOrder(request);
        Assert.Equal(OrderStatus.New, order.Status);
        Assert.Equal(19_000.0, order.Price!.Value, 9);
        Assert.Equal(1.0, order.Quantity, 9);
    }
}
