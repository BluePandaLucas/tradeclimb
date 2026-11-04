#!/usr/bin/env python3
"""Read-only Bitvavo spot-book sanity check; never submits orders.

Usage: python3 scripts/cost-audit.py > docs/cost-audit.json
Static depth walks exclude fees, latency, cancellations, and subsequent impact.
Buy spends EUR notional; sell uses base quantity worth that notional at midpoint.
"""
import datetime
import decimal
import hashlib
import json
import statistics
import time
import urllib.request

D = decimal.Decimal
MARKETS = ['BTC-EUR', 'ETH-EUR', 'SOL-EUR', 'XRP-EUR', 'ADA-EUR']
NOTIONALS = [1000, 2000, 10000]
BASE = 'https://api.bitvavo.com/v2'


def fetch(path):
    started = time.time()
    request = urllib.request.Request(BASE + path, headers={'User-Agent': 'TradeClimb-research/0.1'})
    with urllib.request.urlopen(request, timeout=20) as response:
        raw = response.read()
        server_date = response.headers.get('Date')
    return json.loads(raw), {'url': BASE + path, 'received_utc': datetime.datetime.now(datetime.UTC).isoformat(), 'server_date': server_date, 'request_ms': round((time.time()-started)*1000, 2), 'response_sha256': hashlib.sha256(raw).hexdigest()}


def walk(levels, target, quote_budget):
    remaining = D(target)
    quote = D(0)
    base = D(0)
    for raw_price, raw_size in levels:
        price, size = D(raw_price), D(raw_size)
        take = min(size, remaining / price if quote_budget else remaining)
        quote += take * price
        base += take
        remaining -= take * price if quote_budget else take
        if remaining < D('0.000000000001'):
            return quote / base
    return None  # Never assume sufficient liquidity beyond the returned book.


def main():
    market_data, market_provenance = fetch('/markets')
    metadata = [m for m in market_data if m['market'] in MARKETS]
    samples = []
    for repeat in range(3):
        for market in MARKETS:
            book, provenance = fetch(f'/{market}/book?depth=1000')
            bids, asks = book['bids'], book['asks']
            bid, ask = D(bids[0][0]), D(asks[0][0])
            if bid <= 0 or ask < bid:
                raise ValueError('Invalid or crossed order book')
            mid = (bid + ask) / 2
            costs = []
            for amount in NOTIONALS:
                buy = walk(asks, D(amount), True)
                sell = walk(bids, D(amount)/mid, False)
                costs.append({'notional_eur': amount, 'buy_bps_vs_mid': float((buy/mid-1)*10000) if buy else None, 'sell_bps_vs_mid': float((1-sell/mid)*10000) if sell else None})
            samples.append({**provenance, 'market': market, 'round': repeat+1, 'book_timestamp': book.get('timestamp'), 'nonce': book.get('nonce'), 'bid': str(bid), 'ask': str(ask), 'spread_bps': float((ask-bid)/mid*10000), 'returned_bid_levels': len(bids), 'returned_ask_levels': len(asks), 'costs': costs})
        if repeat < 2:
            time.sleep(2)
    summary = []
    for market in MARKETS:
        for amount in NOTIONALS:
            values = [c[side] for s in samples if s['market']==market for c in s['costs'] if c['notional_eur']==amount for side in ['buy_bps_vs_mid','sell_bps_vs_mid'] if c[side] is not None]
            summary.append({'market':market, 'notional_eur':amount, 'filled_sides':len(values), 'median_bps_vs_mid':statistics.median(values) if values else None, 'min_bps_vs_mid':min(values) if values else None, 'max_bps_vs_mid':max(values) if values else None})
    print(json.dumps({'description':'Short public static-book sanity check, not realized fills or historical cost calibration', 'method':'Buy: walk asks to spend EUR notional before fees. Sell: walk bids for notional/mid base units. Costs relative to contemporaneous midpoint. Each side independent; book unchanged during a walk.', 'limitations':['Only three snapshots per market in one short window; not independent samples or tail estimates','Displayed liquidity can cancel; excludes order latency and subsequent market impact','Backtest uses next candle open, not midpoint, so snapshot costs are not direct historical error estimates','Fees, exchange rounding and order precision are excluded from depth walks','Null costs mean insufficient returned depth, never zero cost'], 'market_provenance':market_provenance, 'markets':metadata, 'summary':summary, 'samples':samples},indent=2))


if __name__ == '__main__':
    main()
