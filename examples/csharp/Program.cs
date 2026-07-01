// Paper-trade differentiator demo. Run: dotnet run
using WickraExchange;

using var ex = Exchange.Paper(
    new Dictionary<string, double> { ["USDT"] = 100_000.0 },
    makerBps: 1.0, takerBps: 5.0, slippageBps: 10.0);
Console.WriteLine($"venue: {ex.Name()}");
ex.SetPrice("BTC/USDT", 20_000.0);

var order = ex.PlaceMarket("BTC/USDT", Side.Buy, 1.0);
Console.WriteLine($"filled at {order.AveragePrice} (status {order.Status})");

foreach (var (asset, free) in ex.Balances())
{
    Console.WriteLine($"  {asset} free: {free}");
}
