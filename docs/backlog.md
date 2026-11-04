# TradeClimb backlog

Pending items await the owner's instruction. Completed UI requests are retained below for reference.

- [x] **Collapsible sidebar.** Allow the left navigation sidebar to collapse and expand.
- [x] **Smoother theme changes.** Add a smooth transition when switching color themes.
- [x] **Overview metrics only on Overview.** Show Strategy versions, Active evaluations, Completed backtests, and Validation budget only on the Overview tab. Remove these duplicate sections from every other tab.
- [x] **Run Backtest bar only on Backtests.** Show the Run Backtest bar only on the Backtests tab; remove it from every other tab, including Overview.
- [x] **Strategy leaderboard.** Add a Leaderboard page showing the best strategies ranked by returns. Rank by campaign objective (daily Sharpe for new campaigns, net CAGR for legacy campaigns) within the same period and exact execution settings, using each version’s latest completed run. Exact ties share rank; unscored runs are excluded, archived versions retained, and validation summaries stay scalar.

- [x] **Clickable evaluation rows.** Open run details by clicking anywhere in a row; retain a keyboard-accessible name button and independent cancellation.
- [x] **Compact tuning results.** Group tuning trials into experiment summaries, hide trial rows by default, and paginate the optional trial list. Keep the selected chart stable while new results arrive.

## Deferred agent research features

Saved at the owner's request; awaiting an instruction to implement.

- [ ] **Standard robustness report.** Expose existing development-only walk-forward analysis through agent tools, with fixed cost-stress presets and separately labeled Kraken comparisons. Return a compact report covering Sharpe, drawdown, benchmark diagnostics, turnover and fold stability; keep validation/final testing owner-controlled.
- [ ] **Shared experiment memory.** Extend existing lineage/idempotency with hypotheses, change explanations, conclusions, model/seed provenance and duplicate detection so agents can find prior experiments.
- [ ] **Per-agent research budgets and fair scheduling.** Add experiment-wide build/evaluation budgets and fair queueing on top of existing per-run limits, so one researcher cannot monopolize the queue.
