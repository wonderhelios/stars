# Reproduction and recovery

Run from `/Users/wonder/Code/stars`. No source or account changes are required.

1. `python3 research/trailing_tp_20261008/retrieve.py` freezes request metadata and retrieves current default `meta` universe, 2 workers, >=200ms request starts, curl 40s timeout, 2 retries. Each coin is atomically written.
2. `python3 research/trailing_tp_20261008/retry_failed.py` retries failed items at >=1s spacing, preserving frozen request end. `retry_proxy.py` uses the authorized local proxy only for failures remaining after direct retry.
3. `python3 research/trailing_tp_20261008/audit_retrieved.py` freezes counts, coverage, hashes, and a recoverable archive. Restore with `tar -xzf research/trailing_tp_20261008/retrieval/data_snapshot.tar.gz -C /tmp`.
4. Create a venv and install `requirements.txt`. Use its Python for `baseline.py`, `verify_factors.py`, and `legacy_baselines.py`. Build the verbatim source factor harness with `rustc research/trailing_tp_20261008/source_factor_check.rs -O -o research/trailing_tp_20261008/retrieval/source_factor_check`.
5. `write_report2.py` renders frozen audit results into REPORT2.md. It does not perform or fabricate any trailing test.

The legacy snapshots are diagnostic provenance for old references, not the source-consistent baseline. The requested trailing grid is gated by `gate_predeclared.json`. Missing held quotes block the gate. Reports mark all unexecuted results as N/A, including p values.
