using WickraExchange;
using Xunit;

namespace WickraExchange.Tests;

public class ExchangeTests
{
    [Fact]
    public void VersionIsExposed()
    {
        Assert.False(string.IsNullOrEmpty(Exchange.Version()));
    }

    [Fact]
    public void PaperMarketBuyFills()
    {
        using var ex = Exchange.Paper(
            new Dictionary<string, double> { ["USDT"] = 100_000.0 },
            makerBps: 1.0, takerBps: 5.0, slippageBps: 10.0);
        Assert.Equal("paper", ex.Name());
        ex.SetPrice("BTC/USDT", 20_000.0);

        var order = ex.PlaceMarket("BTC/USDT", Side.Buy, 1.0);
        Assert.True(order.IsFilled);
        // 10 bps slippage on a buy: 20000 * 1.001 = 20020.
        Assert.NotNull(order.AveragePrice);
        Assert.True(Math.Abs(order.AveragePrice!.Value - 20_020.0) < 1e-6);

        Assert.True(Math.Abs(ex.Balance("BTC") - 1.0) < 1e-9);
        Assert.True(Math.Abs(ex.Balance("USDT") - (100_000.0 - 20_020.0 - 10.01)) < 1e-6);
    }

    [Fact]
    public void EventsAndRestingLimit()
    {
        using var ex = Exchange.Paper(new Dictionary<string, double> { ["USDT"] = 100_000.0 });
        ex.SetPrice("BTC/USDT", 20_000.0);

        var resting = ex.PlaceLimit("BTC/USDT", Side.Buy, 1.0, 19_000.0);
        Assert.Equal(OrderStatus.New, resting.Status);
        var events = ex.Poll();
        Assert.Contains(events, e => e.Kind == EventKind.OrderUpdate);
        ex.Cancel("BTC/USDT", resting.Id);
        Assert.True(Math.Abs(ex.Balance("USDT") - 100_000.0) < 1e-9);
    }

    [Fact]
    public void ReplayParity()
    {
        // A rising tape crosses a 3-period SMA; the market buy fills.
        double[] tape = { 100.0, 101.0, 102.0, 110.0, 112.0 };
        using var ex = Exchange.ReplayTrades(
            "BTC/USDT", tape, new Dictionary<string, double> { ["USDT"] = 100_000.0 });
        Assert.Equal("replay", ex.Name());

        var window = new double[3];
        int seen = 0;
        bool bought = false;

        while (true)
        {
            var events = ex.Poll();
            if (events.Count == 0)
            {
                break;
            }
            foreach (var ev in events)
            {
                if (!ev.IsTrade)
                {
                    continue;
                }
                double price = ev.Price!.Value;
                window[seen % 3] = price;
                seen++;
                if (seen >= 3)
                {
                    double mean = (window[0] + window[1] + window[2]) / 3.0;
                    if (!bought && price > mean)
                    {
                        var order = ex.PlaceMarket("BTC/USDT", Side.Buy, 1.0);
                        Assert.True(order.IsFilled);
                        bought = true;
                    }
                }
            }
        }

        Assert.True(bought);
        Assert.True(Math.Abs(ex.Balance("BTC") - 1.0) < 1e-9);
    }

    [Fact]
    public void MarketDataReads()
    {
        using var ex = Exchange.Paper(new Dictionary<string, double> { ["USDT"] = 100_000.0 });
        ex.SetPrice("BTC/USDT", 20_000.0);

        // ticker reflects the mark on both sides.
        var ticker = ex.Ticker("BTC/USDT");
        Assert.Equal("BTC/USDT", ticker.Symbol);
        Assert.True(Math.Abs(ticker.Last - 20_000.0) < 1e-9);

        // subscribe_* are accepted by the paper feed.
        ex.SubscribeTrades("BTC/USDT");
        ex.SubscribeBook("BTC/USDT");
        ex.SubscribeTicker("BTC/USDT");

        // paper has no historical / depth feed: both throw.
        Assert.Throws<WickraException>(() => ex.Klines("BTC/USDT", "1m", 10));
        Assert.Throws<WickraException>(() => ex.OrderBook("BTC/USDT", 10));

        // A resting limit can be read back by id and appears in open orders.
        var resting = ex.PlaceLimit("BTC/USDT", Side.Buy, 1.0, 19_000.0);
        Assert.Equal(OrderStatus.New, resting.Status);

        var queried = ex.QueryOrder("BTC/USDT", resting.Id);
        Assert.Equal(resting.Id, queried.Id);

        var opens = ex.OpenOrders();
        Assert.Single(opens);
        Assert.Equal(resting.Id, opens[0].Id);
        Assert.Single(ex.OpenOrders("BTC/USDT"));
        Assert.Empty(ex.OpenOrders("ETH/USDT"));
    }
}
