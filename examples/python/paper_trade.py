"""Paper-trade differentiator demo. Run: python paper_trade.py"""

import wickra_exchange as wx


def main() -> None:
    ex = wx.Exchange.paper({"USDT": 100_000.0}, maker_bps=1.0, taker_bps=5.0, slippage_bps=10.0)
    print("venue:", ex.name())
    ex.set_price("BTC/USDT", 20_000.0)

    order = ex.place_order(wx.OrderRequest.market_buy("BTC/USDT", 1.0))
    print(f"filled at {order['average_price']} (status {order['status']})")

    for asset, free in ex.balances().items():
        print(f"  {asset} free: {free}")

    for event in ex.poll_events():
        print("event:", event["type"])


if __name__ == "__main__":
    main()
