# Status and scope

TradeClimb targets this Linux host, one private owner and strategy-development agents. It remains a research and paper-trading tool; strategy generation/orchestration belongs in a separate project.

## Implemented

- Daily Sharpe ranking for new campaigns, preserving legacy CAGR results and existing risk eligibility. Five-asset buy-and-hold is the primary diagnostic comparison, BTC secondary; information ratio and Sharpe difference do not replace ranking.
- Mandatory strategy/build sandboxes, private storage, owner sessions and restricted expiring/revocable agent credentials. A separate-account launcher exposes only the development Unix API and private workspace.
- Pinned agent author kit, machine-readable capabilities and standard-library client. Agents can submit Rust source for offline build, deterministic synthetic preflight and immutable registration, then inspect bounded development candles, decision/fill traces and compiler/runtime diagnostics. See [agent workflow](agents.md).
- Owner-controlled development walk-forward selection and an explicit one-use final-test workflow. Validation remains a globally budgeted scalar result until an audited owner reveal; agents cannot see it after disclosure.
- Separate accepted Kraken data for research comparison and a forward public-book recorder for later execution calibration. The original Kraken campaign was restored into the persistent service on 5 November 2026; select **Kraken 3a1a4d25** in the evaluation campaign dropdown. See [campaign identity and restoration](operations.md#kraken-campaign-in-the-persistent-service). Historical execution assumptions have not been silently changed.
- Searchable strategy/version pickers, history navigation and theme controls with role-scoped access. Run details use translucent solid benchmarks and suppress dense markers while retaining exact trades.

## Approved research policy

For the full accepted Bitvavo snapshot:

| Window | UTC interval | Access |
| --- | --- | --- |
| Development | `[2022-11-01, 2025-11-01)` | Agent research and owner |
| Validation | `[2025-11-01, 2026-08-01)` | Owner, ten global queries by default |
| Final test | `[2026-08-01, 2026-11-01)` | Owner's explicit one-use CLI workflow |

The owner confirmed the final window was untouched. Ordinary evaluations, report/history routes and warmup exclude it, including legacy campaigns. No real final evaluation was performed by implementation QA.

The owner explicitly accepted documented Bitvavo gaps and separately accepted Kraken's April 2024 gaps for research comparison. Keep immutable identities, warnings and distinct venues; never repair Bitvavo prices with Kraken observations. Data stays free for now; no purchase is authorized. Historical model knowledge remains an accepted limitation, not proof of predictive ability.

See [ranked data options](research.md#data-source-shortlist--checked-4-november-2026), [Bitvavo audit](v2-data-audit.json), [Kraken audit](v2-kraken-audit.json) and [forward-book probe](v2-forward-books-audit.json). Audits describe their recording date; old zero-query counts are not live workspace status.

## Deferred work

The next three agent additions are saved, awaiting instruction, in [the backlog](backlog.md#deferred-agent-research-features): standard robustness reports, shared experiment memory, and per-agent research budgets/fair scheduling.

Later extensions include historical universes/delistings, stronger multiple-testing diagnostics, point-in-time model/features, calibrated partial fills/latency/queues and exchange precision. Consider another engine or distributed packaging when those requirements justify it. Real exchange execution requires a separate explicit review.

[Operations](operations.md) describes current controls; [API](api.md) defines contracts; [validation](validation.md) records checks and practical limits. Original planning documents remain historical records.
