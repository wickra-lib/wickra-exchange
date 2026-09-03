using System.Diagnostics;
using WickraExchange;

// What it costs to reach the library from C#.
//
// Same two operations, same offline paper account, same iteration count as
// every other program in this directory and as the Rust baseline. The
// difference from the baseline is this binding's overhead.

const int Iterations = 20_000;
const int Warmup = 1_000;

static void Report(string operation, long nanos)
{
    double perCall = (double)nanos / Iterations;
    Console.WriteLine($"{operation,-12} {perCall,10:F0} ns/op   {1e9 / perCall,12:F0} ops/s");
}

static long Measure(int iterations, Action work)
{
    var watch = Stopwatch.StartNew();
    for (int i = 0; i < iterations; i++)
    {
        work();
    }
    watch.Stop();
    return (long)(watch.Elapsed.TotalMilliseconds * 1_000_000);
}

var ex = Exchange.Paper(new Dictionary<string, double> { ["USDT"] = 1e9 });
ex.SetPrice("BTC/USDT", 20_000.0);

// The first call through any boundary pays for one-time setup, which is not
// what is being measured.
Measure(Warmup, () => ex.Ticker("BTC/USDT"));
Report("ticker", Measure(Iterations, () => ex.Ticker("BTC/USDT")));

var request = new OrderRequest("BTC/USDT", Side.Buy, OrderType.Market, 0.0001);
Measure(Warmup, () => ex.PlaceOrder(request));
Report("place_order", Measure(Iterations, () => ex.PlaceOrder(request)));
