# Benchmark results

- Machine: Apple M3 Max (14 cores), macOS, loopback
- Dart 3.13.7 (AOT, `dart compile exe`), server: `ws_bench_server` (Rust, release)
- Each cell is the median of 3 runs; every run is a fresh process
- Date: 2026-11-11

Variants:

| id | description |
|---|---|
| a | `dart:io` WebSocket on the main isolate, `jsonDecode(String)` |
| b | `dart:io` WebSocket in a worker isolate, decoding in the worker, decoded objects posted to main |
| c | own RFC 6455 codec (`RawSocket`) on main, fused UTF-8+JSON decoder on bytes |
| c-worker | own codec in a worker, decoding in the worker, objects posted to main (batched per socket read) |
| c-worker-raw | own codec in a worker, bytes posted to main (`TransferableTypedData`), decoding on main — models a native (Rust) transport |


### Stream — main isolate µs per message (lower is better)

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 0.75 | 0.03 | 0.60 | 0.05 | 0.39 |
| json 1KB @5k/s | 7.45 | -0.00 | 6.22 | 0.01 | 4.72 |
| json 14KB @1k/s | 87.11 | -0.35 | 71.02 | -0.57 | 58.30 |
| json 60KB @500/s | 322.00 | -0.48 | 264.43 | -0.22 | 242.81 |
| json 60KB unicode @500/s | 576.73 | -0.54 | 275.28 | 0.18 | 244.92 |
| binary 60KB @500/s | 17.41 | -0.65 | 15.97 | 0.07 | -0.36 |
| wss json 1KB @5k/s | 9.14 | 0.07 | 7.97 | -0.00 | 5.08 |
| wss json 14KB @1k/s | 108.70 | 0.73 | 93.96 | 0.58 | 63.60 |
| json 1MB @20/s (stress) | 6267.33 | 72.24 | 5039.25 | 58.38 | 4579.35 |

### Stream — longest main isolate stall, µs

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 379 | 301 | 342 | 326 | 349 |
| json 1KB @5k/s | 216 | 89 | 562 | 95 | 133 |
| json 14KB @1k/s | 366 | 220 | 530 | 209 | 314 |
| json 60KB @500/s | 1492 | 672 | 1239 | 739 | 1120 |
| json 60KB unicode @500/s | 1500 | 644 | 1228 | 655 | 1028 |
| binary 60KB @500/s | 215 | 19 | 237 | 53 | 84 |
| wss json 1KB @5k/s | 266 | 137 | 142 | 93 | 708 |
| wss json 14KB @1k/s | 287 | 184 | 278 | 193 | 248 |
| json 1MB @20/s (stress) | 17657 | 10718 | 16505 | 10843 | 16653 |

### Stream — delivery latency p50 / p99, µs

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 135 / 207 | 137 / 263 | 116 / 189 | 139 / 233 | 139 / 296 |
| json 1KB @5k/s | 126 / 233 | 185 / 312 | 111 / 227 | 173 / 315 | 140 / 276 |
| json 14KB @1k/s | 217 / 425 | 311 / 563 | 196 / 362 | 295 / 514 | 202 / 363 |
| json 60KB @500/s | 444 / 845 | 555 / 1055 | 374 / 690 | 484 / 906 | 381 / 719 |
| json 60KB unicode @500/s | 688 / 1367 | 785 / 1038 | 391 / 772 | 503 / 1010 | 384 / 731 |
| binary 60KB @500/s | 131 / 254 | 141 / 251 | 130 / 241 | 137 / 269 | 141 / 245 |
| wss json 1KB @5k/s | 491 / 858 | 224 / 389 | 496 / 860 | 210 / 332 | 189 / 389 |
| wss json 14KB @1k/s | 892 / 2301 | 333 / 828 | 849 / 2251 | 321 / 787 | 293 / 560 |
| json 1MB @20/s (stress) | 6097 / 8574 | 8420 / 9636 | 4899 / 7004 | 7121 / 8957 | 4969 / 7347 |

### Stream — process CPU µs per message (all threads, probe excluded)

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 1.0 | 1.3 | 0.8 | 1.3 | 1.5 |
| json 1KB @5k/s | 8.4 | 11.6 | 7.1 | 10.4 | 8.3 |
| json 14KB @1k/s | 93.2 | 122.2 | 76.6 | 104.8 | 74.7 |
| json 60KB @500/s | 332.3 | 454.8 | 273.4 | 399.9 | 275.7 |
| json 60KB unicode @500/s | 586.1 | 700.7 | 285.1 | 398.8 | 277.9 |
| binary 60KB @500/s | 25.0 | 34.1 | 24.8 | 33.0 | 35.3 |
| wss json 1KB @5k/s | 13.5 | 21.1 | 12.6 | 19.1 | 18.3 |
| wss json 14KB @1k/s | 152.4 | 220.7 | 136.9 | 209.9 | 185.3 |
| json 1MB @20/s (stress) | 7138.5 | 8791.3 | 5614.8 | 7458.4 | 5494.1 |

### Flood — throughput MB/s (higher is better)

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB | 185.0 | 58.1 | 223.0 | 57.4 | 203.9 |
| json 14KB | 191.4 | 59.7 | 237.1 | 62.3 | 229.8 |
| json 60KB | 189.0 | 52.4 | 241.0 | 67.8 | 255.2 |
| binary 60KB | 6500.8 | 3181.6 | 6775.4 | 3163.0 | 3124.3 |
| wss json 14KB | 106.7 | 83.9 | 118.2 | 88.7 | 177.0 |
| wss binary 60KB | 221.4 | 187.3 | 229.7 | 192.3 | 192.3 |
| json 1MB (stress) | 186.2 | 68.2 | 228.3 | 76.5 | 240.3 |

### Flood — process CPU µs per message

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB | 5.8 | 29.6 | 4.9 | 31.0 | 5.9 |
| json 14KB | 77.7 | 382.5 | 63.4 | 390.8 | 68.1 |
| json 60KB | 337.6 | 1943.0 | 266.6 | 1523.4 | 257.1 |
| binary 60KB | 13.9 | 30.4 | 13.3 | 28.7 | 28.7 |
| wss json 14KB | 142.0 | 195.3 | 129.4 | 187.3 | 171.1 |
| wss binary 60KB | 299.9 | 395.3 | 288.7 | 387.8 | 387.7 |
| json 1MB (stress) | 5873.8 | 24656.2 | 4853.3 | 22194.3 | 4799.7 |

### Flood — peak RSS, MB

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB | 59.5 | 312.7 | 55.7 | 372.8 | 416.3 |
| json 14KB | 62.5 | 337.3 | 62.0 | 316.4 | 587.4 |
| json 60KB | 79.9 | 353.0 | 63.0 | 345.4 | 678.8 |
| binary 60KB | 110.6 | 89.2 | 87.9 | 194.2 | 213.0 |
| wss json 14KB | 28.3 | 59.9 | 28.5 | 28.6 | 29.5 |
| wss binary 60KB | 22.7 | 24.4 | 22.7 | 24.4 | 24.0 |
| json 1MB (stress) | 180.7 | 328.8 | 101.6 | 311.0 | 494.4 |

### Ping-pong — msg/s (MB/s), higher is better

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB | 19063 (20.4) | 16845 (18.0) | 22926 (24.5) | 18985 (20.3) | 19743 (21.1) |
| json 14KB | 3393 (48.6) | 2804 (40.2) | 5098 (73.0) | 3586 (51.4) | 3991 (57.2) |
| json 30KB | 1855 (57.2) | 1514 (46.7) | 2516 (77.5) | 1978 (61.0) | 2203 (67.9) |
| json 60KB | 997 (61.3) | 818 (50.2) | 1368 (84.1) | 1029 (63.3) | 1189 (73.1) |
| binary 60KB | 17158 (1054.2) | 14997 (921.4) | 16159 (992.8) | 13863 (851.7) | 14016 (861.1) |
| wss json 14KB | 2581 (37.0) | 2046 (29.3) | 3202 (45.9) | 2590 (37.1) | 2753 (39.4) |

### Ping-pong — RTT p50 / p99, µs

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB | 50 / 85 | 58 / 91 | 43 / 68 | 52 / 84 | 50 / 78 |
| json 14KB | 291 / 348 | 349 / 452 | 189 / 255 | 275 / 346 | 244 / 296 |
| json 30KB | 535 / 622 | 650 / 821 | 392 / 491 | 498 / 678 | 445 / 589 |
| json 60KB | 994 / 1158 | 1215 / 1452 | 728 / 893 | 956 / 1212 | 831 / 969 |
| binary 60KB | 55 / 98 | 64 / 103 | 60 / 91 | 69 / 112 | 69 / 106 |
| wss json 14KB | 383 / 463 | 481 / 597 | 309 / 386 | 381 / 479 | 359 / 451 |

### Ping-pong — process CPU µs per message

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB | 41.1 | 51.4 | 31.1 | 44.0 | 43.3 |
| json 14KB | 255.8 | 322.7 | 183.5 | 243.5 | 216.0 |
| json 30KB | 498.3 | 629.9 | 355.8 | 470.6 | 420.7 |
| json 60KB | 948.0 | 1175.2 | 686.4 | 926.8 | 799.3 |
| binary 60KB | 46.9 | 61.1 | 48.4 | 63.3 | 62.6 |
| wss json 14KB | 368.2 | 513.6 | 295.6 | 395.4 | 376.2 |

### Echo — RTT p50 / p99, µs

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB @1k/s | 247 / 958 | 316 / 1603 | 353 / 1321 | 459 / 1593 | 504 / 1442 |
| binary 60KB @200/s | 559 / 2501 | 587 / 1313 | 533 / 1568 | 663 / 1535 | 646 / 1728 |

### Echo — process CPU µs per message

| case | a | b | c | c-worker | c-worker-raw |
|---|---:|---:|---:|---:|---:|
| json 1KB @1k/s | 230.0 | 268.5 | 273.1 | 305.9 | 325.2 |
| binary 60KB @200/s | 518.6 | 576.4 | 457.0 | 595.5 | 581.5 |

