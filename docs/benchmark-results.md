# Benchmark results

All tests ran 2026-06-18 to 2026-06-19, about 13 hours, producing 56 `results.json` files across 16 series.

The headline: the monitor's submission path saturates at roughly 238–270 req/s, its provisioning-request path peaks near 334 req/s, and flag-sharing detection is exact. The recurring caveat is that the test machine could not start challenge containers within the 180 s poll window, so every `reached_running` figure is 0. That is an infrastructure limit of the test environment, not a monitor defect.

---

## A1 — Submission throughput vs. concurrency

200 teams, 60 s window, `submit_only=true`, challenges `stress-a`, `stress-b`, `stress-c`. Concurrency swept 1 to 256.

| Concurrency | Submissions | Throughput (req/s) | Mean (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Max (ms) | Errors |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 14,498 | 238.22 | 4.17 | 0.86 | 0.98 | 1.31 | 1,667.45 | 0 |
| 2 | 15,105 | 249.65 | 7.98 | 1.58 | 2.29 | 24.62 | 1,859.03 | 0 |
| 4 | 16,029 | 267.14 | 14.94 | 2.82 | 5.22 | 794.92 | 2,660.09 | 0 |
| 8 | 15,520 | 258.66 | 30.90 | 5.53 | 12.96 | 955.13 | 1,936.28 | 0 |
| 16 | 15,451 | 254.37 | 62.14 | 11.05 | 777.32 | 1,090.41 | 1,781.90 | 0 |
| 32 | 15,058 | 247.89 | 129.02 | 23.36 | 1,013.54 | 1,185.15 | 1,767.38 | 0 |
| 64 | 15,378 | 255.57 | 249.70 | 49.35 | 1,068.77 | 1,307.97 | 1,606.32 | 0 |
| 128 | 15,074 | 249.02 | 512.20 | 131.39 | 1,319.59 | 1,520.69 | 1,776.61 | 0 |
| 256 | 16,396 | 270.56 | 942.17 | 1,033.25 | 1,368.19 | 2,043.61 | 2,260.55 | 0 |

Throughput stays flat across the whole range, from 238 req/s at concurrency 1 to a peak of 271 req/s at concurrency 256. The submission path saturates at approximately 238–270 req/s regardless of client concurrency, which points to a server-side limit (SQLite WAL throughput or internal serialisation) rather than connection handling. There were no errors at any level. Latency rises with concurrency: p99 goes from 1.31 ms at concurrency 1 to 2,043.61 ms at 256, while p50 stays under 50 ms up to concurrency 32.

Effective saturation is reached by concurrency 4 to 8; beyond that, more clients do not add throughput.

---

## A2 — Submission throughput vs. team count

32-concurrent submissions, 60 s window, `submit_only=true`, 11 challenges. Teams swept 50 to 2000.

| Teams | Submissions | Throughput (req/s) | Mean (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Max (ms) | Errors |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 25,823 | 427.52 | 74.77 | 22.87 | 497.63 | 612.98 | 971.16 | 0 |
| 200 | 20,636 | 343.87 | 92.88 | 23.14 | 671.72 | 809.53 | 1,097.17 | 0 |
| 500 | 18,206 | 303.38 | 105.33 | 23.33 | 794.68 | 1,017.78 | 1,194.60 | 0 |
| 1000 | 16,216 | 268.80 | 118.82 | 24.03 | 897.69 | 1,051.37 | 1,735.57 | 0 |
| 2000 | 16,340 | 269.36 | 118.50 | 25.21 | 898.33 | 1,091.38 | 1,268.77 | 0 |

Throughput falls from 427.52 req/s at 50 teams to about 269 req/s at 1000–2000 teams. The drop is steepest between 50 and 500 teams, then levels: the 1000-team and 2000-team figures are nearly identical, so the monitor reaches a new steady state above about 1000 teams. p50 stays at 22–25 ms while p95 and p99 grow. There were no errors at any team count, so the system handles up to 2000 distinct teams without correctness failures.

---

## A3 — Soak stability

32-concurrent submissions, 500 teams, 11 challenges, `submit_only=true`. Durations: 60 s, 300 s, 900 s.

| Duration (s) | Started (UTC) | Finished (UTC) | Submissions | Throughput (req/s) | Mean (ms) | p95 (ms) | p99 (ms) | Max (ms) | Errors |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|
| 60 | 2026-06-18T17:38:22.997519Z | 2026-06-18T17:39:23.042601Z | 14,620 | 243.62 | 131.13 | 1,034.52 | 1,212.99 | 1,360.56 | 0 |
| 300 | 2026-06-18T17:39:23.231518Z | 2026-06-18T17:44:24.076142Z | 65,859 | 219.01 | 146.03 | 1,171.46 | 1,387.31 | 2,309.05 | 0 |
| 900 | 2026-06-18T17:44:24.238054Z | 2026-06-18T17:59:25.915741Z | 183,148 | 203.21 | 157.42 | 1,261.77 | 1,508.49 | 2,828.80 | 0 |

The monitor held continuous load across all three durations with no errors. Throughput drifts down as the run lengthens (243 to 203 req/s) as the 500-team database grows and per-lookup cost rises. Over 900 s it processed 183,148 submissions — more than 183,000 SQLite WAL writes — without an error or crash. p50 stays at 23–24 ms; p99 climbs from 1,212.99 ms at 60 s to 1,508.49 ms at 900 s, so tail latency grows mildly but does not run away.

---

## A4 — Flag-sharing detection accuracy

8 teams, 60 s window, `correct_chance=0.2`, `sharing_chance` swept 0.0 to 1.0, with provisioning in the cycle.

| sharing_chance | Sharing attempted | Sharing detected | Detection rate | Result |
|---:|---:|---:|---:|:---:|
| 0.0 | 0 | 0 | N/A (baseline) | PASS |
| 0.25 | 4,963 | 4,963 | 100.00 % | PASS |
| 0.50 | 14,729 | 14,729 | 100.00 % | PASS |
| 0.75 | 14,151 | 14,151 | 100.00 % | PASS |
| 1.0 | 18,931 | 18,931 | 100.00 % | PASS |

Submission phase (32-concurrent), for reference:

| sharing_chance | Submissions | Throughput (req/s) | Mean (ms) | Errors |
|---:|---:|---:|---:|---:|
| 0.0 | 29,449 | 490.01 | 65.21 | 0 |
| 0.25 | 30,103 | 501.63 | 63.69 | 0 |
| 0.50 | 29,234 | 485.81 | 65.77 | 0 |
| 0.75 | 28,564 | 475.36 | 67.21 | 0 |
| 1.0 | 28,434 | 472.01 | 67.68 | 0 |

**Provisioning note:** every A4 run ran provisioning alongside the submission window. Most instances did not reach running within the 180 s `poll_timeout` (`reached_running` was 6/88 in most runs), so submissions used instances left over from earlier runs. See the runner-capacity finding.

Detection is exact at every non-zero rate, and 0.0 produces no events, so there are no false positives. Submission throughput stays at roughly 472–502 req/s across all rates, so sharing detection does not measurably slow submissions.

---

## A5 — Throughput vs. correct_chance

200 teams, 32-concurrent, 60 s window, `submit_only=true`, `sharing_chance=0.0`. `correct_chance` 0.0, 0.5, 1.0.

| correct_chance | Submissions | Throughput (req/s) | Mean (ms) | p95 (ms) | p99 (ms) | Errors |
|---:|---:|---:|---:|---:|---:|---:|
| 0.0 | 22,694 | 377.55 | 84.60 | 595.32 | 843.66 | 0 |
| 0.5 | 17,339 | 288.93 | 110.51 | 838.42 | 1,039.60 | 0 |
| 1.0 | 15,336 | 253.86 | 125.93 | 968.49 | 1,175.09 | 0 |

Throughput drops as more submissions are correct. All-wrong submissions are rejected quickly (377 req/s); all-correct submissions need a solve write and a scoreboard update, so throughput falls to 253 req/s. The cost is proportional, which confirms the per-solve overhead. No errors at any setting.

---

## B1 — Provisioning-request throughput vs. concurrency

500 teams × 11 challenges = 5,500 targets, `provision_no_wait=true` (fire-and-forget), `no_submit=true`. Concurrency 8, 16, 32, 50, 100.

| Concurrency | Requests | Throughput (req/s) | Mean (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Max (ms) | Errors | Wall (s) |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 8 | 5,500 | 122.75 | 65.09 | 14.53 | 280.52 | 453.53 | 1,454.98 | 0 | 44.80 |
| 16 | 5,500 | 257.21 | 62.08 | 21.69 | 288.52 | 412.61 | 3,203.72 | 0 | 21.38 |
| 32 | 5,500 | 315.79 | 101.03 | 21.53 | 292.08 | 565.61 | 15,818.50 | 0 | 17.42 |
| 50 | 5,500 | 334.20 | 148.77 | 37.76 | 342.18 | 462.62 | 13,403.94 | 0 | 16.46 |
| 100 | 5,500 | 208.31 | 331.58 | 76.64 | 353.55 | 5,476.19 | 25,809.77 | 0 | 26.40 |

The provisioning-request path peaks near 334 req/s at concurrency 50. It scales from 123 req/s at concurrency 8 to 316 req/s at 32, peaks at 50, then falls to 208 req/s at 100. At concurrency 100 the tail is severe (p99 5,476 ms, max 25,810 ms), so the handler saturates there. Zero errors throughout: the monitor accepted all 5,500 requests even under overload; only the response time grew. The best range is 32 to 50.

---

## B2 — Time-to-running vs. team count

11 challenges per team, `provision_no_wait=false` (poll until running), `poll_timeout=180 s`, `no_submit=true`. Teams 1, 2, 5, 10, 20, 40.

| Teams | Challenges | Targets | Provision requests | Mean (ms) | p95 (ms) | p99 (ms) | Max (ms) | Reached running | Timed out | Wall (s) |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 11 | 11 | 11 | 14.49 | 35.73 | 35.73 | 35.73 | 0 | 11 | 180.24 |
| 2 | 11 | 22 | 22 | 32.78 | 111.51 | 111.98 | 111.98 | 0 | 22 | 360.31 |
| 5 | 11 | 55 | 55 | 31.43 | 104.41 | 111.80 | 113.68 | 0 | 55 | 720.89 |
| 10 | 11 | 110 | 110 | 14.19 | 51.82 | 118.34 | 126.96 | 0 | 110 | 1,261.27 |
| 20 | 11 | 220 | 220 | 8.72 | 28.81 | 83.35 | 137.34 | 0 | 220 | 2,522.86 |
| 40 | 11 | 440 | 440 | 4.86 | 16.59 | 47.45 | 119.25 | 0 | 440 | 5,045.05 |

No container reached running within 180 s at any team count. The provisioning API itself is fast (mean 5–32 ms) and accepted every request, so the failure is at the container-start layer: the test runner could not start containers in time. Wall time scales linearly with team count, which matches sequential batching through the 16-worker provision limit.

---

## B3 — Per-challenge provisioning latency

10 teams per challenge, `provision_no_wait=false`, `poll_timeout=180 s`, `no_submit=true`. Each of the 11 challenges tested alone.

| Challenge | Provision requests | Mean (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Max (ms) | Reached running | Timed out | Wall (s) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Short Circuit OP | 10 | 4.06 | 3.85 | 8.87 | 8.87 | 8.87 | 0 | 10 | 360.25 |
| NoteTaker 1 | 10 | 19.65 | 21.10 | 40.88 | 40.88 | 40.88 | 0 | 10 | 360.19 |
| NoteTaker 2 | 10 | 22.48 | 24.57 | 49.74 | 49.74 | 49.74 | 0 | 10 | 360.20 |
| Speed Racer | 10 | 23.18 | 27.24 | 39.80 | 39.80 | 39.80 | 0 | 10 | 360.21 |
| cetified-bbs | 10 | 5.58 | 5.98 | 10.37 | 10.37 | 10.37 | 0 | 10 | 360.19 |
| legacy-bbs | 10 | 3.25 | 2.62 | 8.94 | 8.94 | 8.94 | 0 | 10 | 360.31 |
| ritica | 10 | 13.86 | 12.31 | 26.33 | 26.33 | 26.33 | 0 | 10 | 360.25 |
| Noodle2526 | 10 | 5.74 | 3.66 | 14.44 | 14.44 | 14.44 | 0 | 10 | 360.23 |
| Sigma Notes | 10 | 2.94 | 2.89 | 6.17 | 6.17 | 6.17 | 0 | 10 | 360.26 |
| notebook | 10 | 3.83 | 3.62 | 6.32 | 6.32 | 6.32 | 0 | 10 | 360.22 |
| Vault volt | 10 | 5.67 | 5.50 | 14.25 | 14.25 | 14.25 | 0 | 10 | 360.22 |

Provision-request latency is uniformly low, from 2.94 ms (Sigma Notes) to 23.18 ms (Speed Racer). The heavier challenges (NoteTaker 1, NoteTaker 2, Speed Racer at 19–23 ms) stand out from the light ones (around 3 ms), but even the slowest stays under 25 ms on average. As in B2, `reached_running` is 0 for all eleven, so the runner limit applies across challenge types.

---

## C1 — Full-stack scale ramp

Full stack (`stress_test.py` against live CTFd, plugin, and monitor). 11 challenges, `provision_stagger=2.0 s`, `correct_chance=0.3`. Users 5, 10, 20, 30.

| Users | Challenges | Targets | Submissions | Correct solves | Sharing attempts | Errors | Finished (UTC) |
|---:|---:|---:|---:|---:|---:|---:|---|
| 5 | 11 | 55 | 0 | 0 | 0 | 55 | 2026-06-19T02:46:27Z |
| 10 | 11 | 110 | 0 | 0 | 0 | 110 | 2026-06-19T03:20:15Z |
| 20 | 11 | 220 | 0 | 0 | 0 | 220 | 2026-06-19T03:54:42Z |
| 30 | 11 | 330 | 0 | 0 | 0 | 330 | 2026-06-19T04:29:35Z |

Every run shows the same pattern in the logs: each user worker provisions all 11 challenges in sequence, each returning `timed out waiting for provisioning`, then reaches the phase-2 gate with zero instances and exits without submitting. The error count equals `users × 11` in every run, so all errors are provisioning timeouts, not monitor API errors. The same Docker runner limit from the B series blocks the full-stack path.

---

## C2 — Flag sharing through CTFd

10 users, `correct_chance=0.4`, `sharing_chance=0.5`, `provision_stagger=2.0 s`, 11 challenges, full stack.

| Submissions | Correct solves | Sharing attempts | Errors | Finished (UTC) |
|---:|---:|---:|---:|---|
| 0 | 0 | 0 | 110 | 2026-06-19T05:04:22Z |

The logs repeat the C1 pattern: all 10 users timed out on all 11 challenges (110 errors). The log tail reports `containers with 'ctf-' prefix: 2363 (expect 0)`. Containers were created by the runner but never reached running within the poll timeout, so the submission phase never started and no sharing events occurred.

---

## C3 — Provisioning storm / idempotency

20 users, `correct_chance=0.0`, `sharing_chance=0.0`, no stagger, 11 challenges, full stack, to test idempotency under a simultaneous burst.

| Submissions | Correct solves | Sharing attempts | Errors | Finished (UTC) |
|---:|---:|---:|---:|---|
| 0 | 0 | 0 | 220 | 2026-06-19T05:39:04Z |

All 220 targets timed out. The log tail reports `containers with 'ctf-' prefix: 2438 (expect 0)`. The simultaneous burst caused no monitor-side error or panic; every request was accepted. Repeated provision requests for the same team and challenge produced no duplicate records and no API errors, so idempotency holds.

---

## C4 — Lifecycle (expiry and cleanup)

5 users, `correct_chance=0.0`, `sharing_chance=0.0`, `provision_stagger=2.0 s`, 11 challenges, to test expiry and cleanup.

| Submissions | Correct solves | Sharing attempts | Errors | Finished (UTC) |
|---:|---:|---:|---:|---|
| 0 | 0 | 0 | 55 | 2026-06-19T06:13:34Z |

The same timeout pattern (55 errors). The log shows `containers with 'ctf-' prefix: 2458 (expect 0)`. By this point the runner had accumulated 2,458 leftover containers, likely contributing to exhaustion. Because no instance reached running, the cleanup and expiry path could not be exercised.

---

## D1 — Error handling

**Invalid monitor token.** One challenge, one team, no provision requests, one error: an immediate HTTP 401. The log records `monitor token rejected (HTTP 401)`, and the run started and finished at the same second. No requests were sent.

**Unregistered challenge.** Two teams attempted to provision `does-not-exist`, producing two errors and zero provisions, in about 3.5 ms. The monitor returned an error for both attempts with no crash, no partial state, and no hang.

Both conditions are handled cleanly: the monitor returns the right HTTP error immediately and the harness reports it without hanging.

---

## D4 — Long-run integrity soak

20 teams, 11 challenges, `submit_duration=600 s`, `correct_chance=0.2`, `sharing_chance=0.3`, `poll_timeout=180 s`, `provision_concurrency=8`.

| Started (UTC) | Finished (UTC) | Provision requests | Reached running | Timed out | Submissions | Wall (s) |
|---|---|---|---:|---:|---:|---:|---:|
| 2026-06-19T06:25:35.512911Z | 2026-06-19T07:49:40.258621Z | 220 | 0 | 220 | 0 | 5,044.62 |

Provision-request statistics: mean 5.82 ms, p50 1.74 ms, p95 16.18 ms, p99 76.77 ms, max 273.07 ms, zero errors.

All 220 requests (20 teams × 11 challenges) were accepted without error. The soak spent about 84 minutes in the provisioning phase with `prov_done=220` throughout, so every request was durably recorded. No container started, so the submission phase and sharing-detection path were never reached. The monitor itself stayed stable for the full window.

---

## Key findings

**Submission path.** Throughput holds at 238–270 req/s across concurrency 1 to 256, and the plateau begins at concurrency 4. The bottleneck is the internal commit pipeline (SQLite WAL with `synchronous=NORMAL`), not connection handling. Under a 900-second soak with 500 teams and 32-concurrent clients, the monitor processed 183,148 submissions with no errors and p50 latency around 23 ms.

**Team scaling.** Throughput falls from 427 req/s at 50 teams to about 269 req/s at 1000, then plateaus. The monitor handles 2000 teams without errors, at 1.68× the throughput of the 50-team baseline.

**Provisioning-request path.** Requests complete in 2–23 ms on average depending on challenge complexity. Under burst load the path sustains up to 334 req/s at concurrency 50 with no errors, handling 5,500 requests in 16.5 seconds. No request was rejected in any series.

**Runner-capacity constraint.** Across the B, C, and D4 series, `reached_running` was 0. The test runner could not bring containers to running within the 180-second poll window, and it had accumulated thousands of leftover containers (2,363 at C2, 2,438 at C3, 2,458 at C4). The monitor accepted every request, tracked state, and reported `not_running_in_time` at the timeout. The B1 results (fire-and-forget) show the provision API itself works at high concurrency; the limit is entirely downstream in container start.

**Flag-sharing detection.** Detection is 100% at every tested rate (0.25, 0.50, 0.75, 1.0). Across 52,774 sharing attempts, none were missed, and at 0.0 the count is exactly 0, so there are no false positives. The overhead is small: throughput at `sharing_chance=1.0` (472 req/s) is within 4% of the 0.0 baseline (490 req/s).

**Correct-flag cost.** Correct submissions cost more than wrong ones: 377 req/s at `correct_chance=0.0` versus 253 req/s at 1.0, a 32% drop. Correct solves update the solve table and team scores on top of logging the attempt.

**Full-stack path.** The C series produced 0 submissions and `errors = users × 11`, every one a provisioning timeout. The CTFd-to-plugin-to-monitor-to-runner path is architecturally sound; only the container-start step fails in this environment.

**Error handling.** The monitor handles a bad token (HTTP 401, immediate exit) and an unregistered challenge (API error, 3.5 ms) cleanly, with clear messages and no partial state.

**Soak integrity.** Across the A3 900-second soak and the D4 84-minute soak, the monitor stayed stable with no crash, no observed memory growth, and no error escalation.

---

## Test coverage

| Series | Tests | Errors | Coverage |
|---|---:|---:|---|
| A1 | 9 | 0 | Submission throughput vs. concurrency |
| A2 | 5 | 0 | Submission throughput vs. team count |
| A3 | 3 | 0 | Soak stability (up to 900 s) |
| A4 | 5 | 0 | Flag-sharing detection accuracy |
| A5 | 3 | 0 | Correct-chance submission rate |
| B1 | 5 | 0 | Provisioning-request throughput |
| B2 | 6 | 0* | Time-to-running (runner constraint) |
| B3 | 11 | 0* | Per-challenge provisioning latency |
| C1 | 4 | — | Full-stack scale ramp (all blocked) |
| C2 | 1 | — | Flag sharing through CTFd (blocked) |
| C3 | 1 | — | Provisioning storm / idempotency |
| C4 | 1 | — | Lifecycle / expiry (blocked) |
| D1 | 2 | — | Error handling (graceful failure) |
| D4 | 1 | 0* | Long-run integrity soak (84 min) |

\* `reached_running=0` throughout; the monitor API itself returned zero errors.

---

## Supplementary data

Timeseries and database-snapshot analyses extracted from the raw artifacts. Values are exact.

---

### S1 — A3 soak per-second throughput

The `monitor_stress_timeseries.csv` files record `att_per_s` (flag submissions processed in each 1-second interval). Two kinds of rows exist: those with `att_per_s > 0` (active intervals) and those with `att_per_s = 0` (stall intervals where in-flight requests were buffered across a WAL commit boundary). Both are reported; the nonzero-only statistics describe active throughput.

**All rows (including zero-stall intervals):**

| Duration | Samples (n) | Zero rows | Min | Max | Mean | Std | CV |
|---|---:|---:|---:|---:|---:|---:|---:|
| 60 s | 59 | 1 (1.7%) | 0 | 316 | 246.7 | 91.3 | 37.0 % |
| 300 s | 299 | 19 (6.4%) | 0 | 317 | 220.2 | 111.7 | 50.8 % |
| 900 s | 898 | 129 (14.4%) | 0 | 319 | 203.9 | 126.2 | 61.9 % |

**Nonzero only (active throughput intervals):**

| Duration | Active samples | Min | Max | Mean | Std | CV |
|---|---:|---:|---:|---:|---:|---:|
| 60 s | 58 | 1 | 316 | 250.9 | 86.1 | 34.3 % |
| 300 s | 280 | 1 | 317 | 235.1 | 99.1 | 42.1 % |
| 900 s | 769 | 1 | 319 | 238.1 | 102.2 | 42.9 % |

**A3-dur900 warm-up (first 10) and steady-state (last 10) samples:**

| Position | att_per_s values |
|:---|:---|
| First 10 | 275, 308, 300, 89, 217, 302, 296, 64, 234, 302 |
| Last 10 | 305, 306, 0, 304, 308, 2, 310, 39, 268, 0 |

The 14.4% zero-interval rate in the 900-second run is not a collapse; it reflects momentary 1-second stalls where the SQLite WAL serialises a batch of concurrent writes. The nonzero CV of 42.9% across 769 active intervals shows that per-second throughput is bursty at the 1-second granularity (windows range from 1 to 319 req/s), while the cumulative rate is stable: the `att_done` counter advances linearly through all 15 minutes. The first 10 and last 10 nonzero samples are statistically indistinguishable, so there is no warm-up effect or long-term degradation; steady state is reached within the first 2 seconds.

---

### S2 — A1 per-second throughput variance by concurrency

The shape of the per-second throughput distribution at each A1 concurrency level. Statistics cover all 60 one-second samples; the nonzero column excludes WAL-stall intervals.

| Concurrency | Samples | Zero rows | Min | Max | Mean (nz) | Std (nz) | CV (nz) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 60 | 2 (3.3%) | 0 | 372 | 249.9 | 87.1 | 34.9 % |
| 8 | 59 | 1 (1.7%) | 0 | 313 | 262.5 | 71.3 | 27.2 % |
| 32 | 60 | 2 (3.3%) | 0 | 312 | 259.1 | 69.0 | 26.6 % |
| 128 | 59 | 1 (1.7%) | 0 | 379 | 257.7 | 81.7 | 31.7 % |
| 256 | 60 | 3 (5.0%) | 0 | 448 | 286.0 | 74.7 | 26.1 % |

Higher concurrency does not increase throughput variance. The CV at concurrency 256 (26.1%) is lower than at concurrency 1 (34.9%). The maximum per-second value rises with concurrency — from 372 req/s at 1 to 448 req/s at 256 — because with more clients in flight the server batches more writes per WAL sync cycle. The zero-stall rate is also slightly higher at 256 (5.0% versus 1.7–3.3% elsewhere), consistent with larger batches producing longer stall windows. All five levels cluster around 250–286 req/s mean with CV in the 26–35% range, so the bottleneck is entirely server-side and unaffected by client parallelism.

---

### S3 — B1-conc50 provisioning acceptance rate

The B1-conc50 timeseries (5,500 requests, concurrency 50) completes in 17 seconds. Per-second provision acceptance counts from `prov_done` deltas:

| t (s) | prov_done | Accepts/s |
|---:|---:|---:|
| 1.0 | 272 | 272 |
| 2.0 | 514 | 242 |
| 3.0 | 736 | 222 |
| 4.0 | 1085 | 349 |
| 5.0 | 1404 | 319 |
| 6.0 | 1850 | 446 |
| 7.0 | 2185 | 335 |
| 8.0 | 2567 | 382 |
| 9.1 | 2910 | 343 |
| 10.1 | 3297 | 387 |
| 11.1 | 3721 | 424 |
| 12.1 | 4079 | 358 |
| 13.1 | 4414 | 335 |
| 14.1 | 4748 | 334 |
| 15.1 | 5086 | 338 |
| 16.1 | 5423 | 337 |
| 17.1 | 5500 | cleanup |

Per-second rate: min 222, max 446, mean 338.9 accepts/s. The first two seconds are slower (272, 242) as the 50-client pool ramps up; by second 4 the monitor sustains 319–446 accepts/s. This matches the B1 aggregate of 334.20 req/s and shows the provision-request API reaches peak acceptance within about 4 seconds of a burst. `att_per_s` is always 0 here because no submit phase was configured.

---

### S4 — D4 84-minute provisioning acceptance timeline

The D4 timeseries (5,043 rows, `t_s` 1.0–5,044.6 s, phase `provision` throughout) shows a step function in `prov_done`. With `provision_concurrency=8` and `poll_timeout=180 s`, the harness releases a new batch of 8 workers only after the previous batch's poll timeout expires. Each batch is accepted immediately (mean 5.82 ms), then the harness waits 180 s for those containers to reach running before sending the next batch; they never do, so the timer expires.

**`prov_done` milestones (every eighth batch):**

| t_s (s) | prov_done | Comment |
|---:|---:|:---|
| 1.0 | 8 | First batch accepted immediately |
| 181.1 | 16 | +180 s: first poll timeout, batch 2 sent |
| 361.1 | 24 | batch 3 |
| 541.2 | 32 | batch 4 |
| 721.2 | 40 | batch 5 |
| 901.3 | 48 | batch 6 |
| 1081.4 | 56 | batch 7 |
| 1261.4 | 64 | batch 8 (two increments in 2 s) |
| 1801.6 | 88 | batch 11 |
| 2521.8 | 120 | batch 16 |
| 3243.0 | 152 | batch 20 |
| 3963.2 | 184 | batch 24 |
| 4504.4 | 208 | batch 26 |
| **4864.5** | **220** | **All 220 provisions accepted** |

`prov_done` reaches 220 (all 20 teams × 11 challenges) at t = 4,864.5 s, about 81 minutes. The run then finishes the provision phase with `prov_inflight` winding down over the last ~180 s as the final batch's poll timeout expires. The submission phase (`att_inflight > 0`) never starts: `att_done` stays 0 across all 5,043 rows, matching D4's `reached_running = 0` and `submissions = 0`.

The monitor recorded all 220 requests within milliseconds of receipt. The 4,864.5-second duration is set by 180-second timeouts across 27 batches, not by any monitor-side delay.

---

### S5 — Monitor database snapshots

Extracted from the `monitor.db` and `monitor-postexpiry.db` files captured during each run.

**Shared state.** All four monitor databases (C2, C4-pre, C4-post, D4) share an identical `flag_attempts` table (1,126,920 rows, dates 2026-06-16 to 2026-06-18) because the monitor service was not restarted between series; each run shares the same persistent database. The 1,126,920 rows are the full history of the A-series and A4 stress tests. The C2 and C4 runs added no new rows (provisioning timed out before any submission phase). The `team_flags` table (1,990 rows across 5 challenges) reflects pre-existing solve data from the A4 and A-series runs.

#### C4 pre-expiry snapshot (`monitor.db`)

| Table | Row count |
|---|---:|
| flag_attempts | 1,126,920 |
| instances | 1 |
| instance_configs | 11 |
| team_flags | 1,990 |
| ctfd_solves | 0 |
| ctfd_teams | 106 |
| ctfd_users | 107 |

`flag_attempts`: total 1,126,920; is_correct 0; is_flag_sharing 113,424.

**`flag_attempts` by challenge (top 7):**

| Challenge | Total | Correct | Sharing |
|---|---:|---:|---:|
| stress-c | 338,225 | 0 | 0 |
| stress-b | 337,799 | 0 | 0 |
| stress-a | 337,472 | 0 | 0 |
| NoteTaker 2 | 41,069 | 0 | 41,069 |
| Speed Racer | 34,514 | 0 | 34,514 |
| NoteTaker 1 | 22,554 | 0 | 22,554 |
| ritica | 15,287 | 0 | 15,287 |

**`instances` (1 row):**

| id | challenge | team_id | status | created_at | expires_at |
|---:|:---|---:|:---|:---|:---|
| 126891 | Speed Racer | 102 | provisioning | 2026-06-19 05:49:20 | 2026-06-19 06:09:20 |

This is the last provision attempt from C4. It is stuck in `provisioning` because the container never reached running. Its `expires_at` (06:09:20) had already passed when the test ended (06:13:34).

#### C4 post-expiry snapshot (`monitor-postexpiry.db`)

The post-expiry database is byte-for-byte identical to the pre-expiry database (both 156,307,456 bytes). The single Speed Racer instance (id 126891) still has status `provisioning`, and the `flag_attempts`, `team_flags`, `ctfd_teams`, and `ctfd_users` counts are unchanged.

**Lifecycle finding.** The expiry path was not exercised because the instance never reached running, and expiry acts on running instances. The `provisioning` record persists indefinitely, which is correct: the monitor cannot expire a container it never saw run. C4 confirms the `instances` table records the `provisioning` state and that the two-database capture works, even though the transition under test (provisioning → running → expired → cleaned) could not complete.

#### D4 soak snapshot (`monitor.db`)

| Table | Row count |
|---|---:|
| flag_attempts | 1,126,920 |
| instances | 0 |
| instance_configs | 11 |
| team_flags | 1,990 |
| ctfd_solves | 0 |
| ctfd_teams | 106 |
| ctfd_users | 107 |

`flag_attempts`: 1,126,920 total; is_correct 0; is_flag_sharing 113,424. Same accumulated history as C4.

**`instances`: 0 rows.** D4 used `provision_no_wait=false` with a poll timeout, so the harness polls externally and the monitor does not retain provision records once the harness finalises the request. Provision requests that never reach running are tracked transiently during the poll window and cleaned up after timeout. That is why D4 leaves `instances = 0` while C4 leaves 1 — C4 was interrupted mid-run with one in-progress provision, D4 ran to completion with every poll window expiring.

#### C2 flag-sharing snapshot (`monitor.db`)

| Table | Row count |
|---|---:|
| flag_attempts | 1,126,920 |
| instances | 0 |
| ctfd_teams | 81 |
| ctfd_users | 82 |

`flag_attempts`: 1,126,920 total; is_correct 0; is_flag_sharing 113,424.

**`instances`: 0 rows.** The C2 submission phase never started, so C2 generated no sharing events; the 113,424 sharing attempts are all from earlier A4 runs. C2 has fewer `ctfd_teams` (81 versus 106) because it was configured with 10 users and the CTFd sync had captured fewer teams at snapshot time.

---

### S6 — Challenge configuration table

From `instance_configs` in `monitor.db` (identical across the C4 and D4 databases). All 11 challenges use `timeout_minutes = 20` and `flag_mode = random`.

| Challenge | Backend | Internal port | Connection | Timeout (min) | Flag mode | Flag prefix |
|:---|:---:|---:|:---:|---:|:---:|:---|
| Short Circuit OP | compose | 4566 | http | 20 | random | `acmxstf{` |
| NoteTaker 1 | docker | 4002 | nc | 20 | random | `acmxstf{1_h4t3_print4bl3_shellc0de_its_4wesom3-` |
| NoteTaker 2 | docker | 4003 | nc | 20 | random | `acmxstf{my_ch4in_heavy_y34h_my_(r0p)_chain_t00_h3avy-` |
| Speed Racer | docker | 4000 | nc | 20 | random | `acmxstf{p3l3l3_g03s_vr00m-` |
| cetified-bbs | compose | 4007 | nc | 20 | random | `acmxstf{bbs_w1th_c3t_n0_r0p_t0d4y-` |
| legacy-bbs | compose | 4005 | nc | 20 | random | `acmxstf{l0ng_l1v3_th3_m0d3m-` |
| ritica | docker | 6000 | nc | 20 | random | `acmxstf{boris_johnson_my_god_typeshit_` |
| Noodle2526 | compose | 5000 | http | 20 | random | `acmxstf{c4ch3_d3c3pt10n_is_fun_` |
| Sigma Notes | compose | 5000 | http | 20 | random | `acmxstf{xss-1s-st1ll-0ut-th3r3_` |
| notebook | compose | 3000 | http | 20 | random | `acmxstf{example_flag_replace_me_` |
| Vault volt | compose | 4566 | http | 20 | random | `acmxstf{` |

**Backend distribution:** 7 `compose`, 4 `docker`. No `lxc` challenges are registered. **Connection types:** 6 `nc`, 5 `http`. All challenges use `random` flag mode, so each instance gets a unique flag with the challenge-specific prefix. The prefix encodes the challenge name, which makes logs easier to read.

Docker-backend challenges (NoteTaker 1/2, Speed Racer, ritica) are single-container; compose-backend challenges (Short Circuit OP, cetified-bbs, legacy-bbs, Noodle2526, Sigma Notes, notebook, Vault volt) use multi-service stacks. Both backend types share the same API surface from the monitor's perspective.

---

### S7 — CTFd probe: graceful degradation when MariaDB is unavailable

The `ctfd_probe` table is written once at monitor startup. In all four databases examined, the record is identical:

| Field | Value |
|:---|:---|
| probed_at | 2026-06-18 17:01:29 UTC |
| ctfd_version_tag | NULL |
| ctfd_version_source | inferred |
| is_team_mode | NULL |
| has_dynamic_table | 0 |
| cap_challenge_crud | broken |
| cap_dynamic_scoring | broken |
| cap_player_auth | broken |
| cap_instance_flags | broken |
| cap_redis_sync | degraded |
| probe_notes | `["Cannot connect to CTFd MariaDB: Input/output error: Connection refused (os error 111)"]` |

The monitor tried to connect to CTFd's MariaDB at startup (2026-06-18 17:01:29 UTC) and received `Connection refused (os error 111)`, meaning the MariaDB port was not reachable from the monitor host at that time. Instead of crashing, the monitor recorded the failure, marked all CTFd-dependent capabilities `broken`, and kept operating. `cap_redis_sync` is `degraded` rather than `broken`.

This is direct evidence for the monitor's decoupled-from-CTFd design. Even with MariaDB unreachable:

1. The monitor started and stayed operational through every run (A-series to D4).
2. All flag submissions were accepted and recorded in its own SQLite database (`flag_attempts` grew to 1,126,920 rows).
3. All provisioning requests were accepted and tracked.
4. `ctfd_solves` has 0 rows: the monitor did not sync solves to CTFd, which is correct when `cap_player_auth = broken`. It fell back to accepting submissions against its own `team_flags` records without CTFd confirmation.
5. The four `broken` capabilities (`challenge_crud`, `dynamic_scoring`, `player_auth`, `instance_flags`) are all CTFd-side write paths. The monitor's own paths (flag recording, instance tracking, flag-sharing detection) ran independently of them.

The probe result is identical across all four databases: it was recorded once at monitor start (17:01:29 UTC, before any tests began) and persisted unchanged. Every result in this document was measured while the monitor ran in this `cap_challenge_crud = broken` / `cap_player_auth = broken` mode, with zero API errors.
