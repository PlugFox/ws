# Benchmark results

- Machine: Apple M3 Max (14 cores), macOS, loopback
- Dart 3.13.7 (AOT, `dart build cli`), Rust 1.99 (release, thin LTO), server: `ws_bench_server` (Rust, release)
- Each cell is the median of 3 runs; every run is a fresh process
- Date: 2026-11-12

Variants:

| id | description |
|---|---|
| a | `dart:io` WebSocket on the main isolate, `jsonDecode(String)` |
| b | `dart:io` WebSocket in a worker isolate, decoding in the worker, decoded objects posted to main |
| c | own RFC 6455 codec (`RawSocket`) on main, fused UTF-8+JSON decoder on bytes |
| c-worker | own codec in a worker, decoding in the worker, objects posted to main (batched per socket read) |
| c-worker-raw | own codec in a worker, bytes posted to main (`TransferableTypedData`), decoding on main |
| c-tape | own codec on main, JSON → tape, lazy `TapeMap` views |
| c-worker-tape | own codec in a worker that builds the tape; tape moved to main, lazy views |
| c-worker-model | own codec in a worker that decodes **and maps to typed models**; models posted to main |
| d-bytes | Rust (tokio + tungstenite + rustls) via FFI, payloads as external typed data, decoding on main |
| d-tape | Rust, simd-json → tape (same format), external typed data, lazy views on main |
| d-cobject | Rust, simd-json → `Dart_CObject` tree, rebuilt by the VM on main |

`→ model` cases map every message to typed models (`Order.fromJson`, all fields).



### Stream — main isolate µs per message (lower is better)

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 0.68 | 0.04 | 0.67 | 0.03 | 0.40 | 0.61 | 0.17 | 0.02 | 0.43 | 0.22 | 0.22 |
| json 1KB @5k/s | 7.05 | -0.02 | 6.03 | -0.01 | 4.98 | 5.71 | 0.40 | -0.18 | 5.54 | 0.46 | 4.00 |
| json 1KB @5k/s → model | 9.53 | 1.20 | 8.52 | 1.20 | 5.81 | 7.29 | 2.26 | -0.10 | 5.91 | 2.50 | 5.63 |
| json 14KB @1k/s | 84.78 | -0.46 | 70.32 | -0.44 | 58.12 | 55.67 | -0.13 | -0.45 | 61.19 | 0.59 | 44.36 |
| json 14KB @1k/s → model | 97.87 | 12.57 | 82.74 | 12.60 | 71.16 | 84.26 | 27.18 | -0.60 | 72.30 | 29.21 | 71.38 |
| json 60KB @500/s | 321.44 | -0.97 | 262.22 | -0.17 | 238.05 | 219.58 | 0.17 | -0.68 | 246.18 | 1.34 | 187.29 |
| json 60KB @500/s → model | 377.28 | 54.68 | 316.02 | 53.63 | 298.97 | 335.96 | 128.80 | -0.42 | 298.40 | 119.76 | 305.33 |
| json 60KB unicode @500/s → model | 615.69 | 47.50 | 318.47 | 45.07 | 297.59 | 309.04 | 128.34 | -0.46 | 296.98 | 133.09 | 259.52 |
| binary 60KB @500/s | 15.99 | -0.93 | 15.38 | -0.86 | -1.09 | 15.51 | -0.93 | -0.85 | 0.18 | 0.25 | 0.24 |
| wss json 14KB @1k/s → model | 119.61 | 15.74 | 102.43 | 15.74 | 76.21 | 104.45 | 32.23 | 0.26 | 72.30 | 28.87 | 71.36 |
| json 1MB @20/s → model (stress) | 7527.65 | 1462.78 | 6309.59 | 1386.09 | 5772.77 | 6183.27 | 2272.62 | 178.02 | 5768.41 | 2130.18 | 6467.02 |

### Stream — longest main isolate stall, µs

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 199 | 172 | 193 | 176 | 199 | 226 | 180 | 184 | 187 | 347 | 694 |
| json 1KB @5k/s | 207 | 69 | 162 | 42 | 2852 | 396 | 197 | 70 | 262 | 378 | 175 |
| json 1KB @5k/s → model | 283 | 145 | 761 | 111 | 95 | 187 | 56 | 17 | 194 | 263 | 653 |
| json 14KB @1k/s | 381 | 196 | 292 | 209 | 260 | 292 | 226 | 217 | 274 | 252 | 348 |
| json 14KB @1k/s → model | 339 | 218 | 431 | 221 | 1297 | 346 | 255 | 238 | 334 | 599 | 421 |
| json 60KB @500/s | 1287 | 740 | 1175 | 703 | 1191 | 1217 | 781 | 803 | 1151 | 865 | 1356 |
| json 60KB @500/s → model | 1481 | 720 | 1386 | 756 | 1175 | 1300 | 906 | 779 | 1275 | 1078 | 1679 |
| json 60KB unicode @500/s → model | 1660 | 684 | 1172 | 639 | 1387 | 1233 | 1586 | 673 | 1816 | 976 | 1360 |
| binary 60KB @500/s | 117 | 169 | 125 | 23 | 28 | 102 | 32 | 13 | 123 | 115 | 154 |
| wss json 14KB @1k/s → model | 273 | 207 | 303 | 212 | 299 | 314 | 264 | 230 | 335 | 302 | 385 |
| json 1MB @20/s → model (stress) | 18620 | 11803 | 17811 | 11680 | 17158 | 18535 | 14927 | 12559 | 17465 | 14843 | 23924 |

### Stream — delivery latency p50 / p99, µs

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 134 / 206 | 145 / 258 | 129 / 195 | 140 / 244 | 146 / 308 | 135 / 215 | 157 / 296 | 147 / 251 | 134 / 220 | 143 / 225 | 157 / 285 |
| json 1KB @5k/s | 154 / 251 | 195 / 307 | 148 / 238 | 193 / 302 | 152 / 339 | 114 / 254 | 165 / 396 | 187 / 298 | 138 / 270 | 133 / 243 | 287 / 449 |
| json 1KB @5k/s → model | 143 / 277 | 203 / 392 | 133 / 277 | 191 / 403 | 146 / 270 | 133 / 235 | 168 / 266 | 188 / 311 | 141 / 236 | 144 / 233 | 278 / 411 |
| json 14KB @1k/s | 228 / 419 | 306 / 509 | 206 / 329 | 276 / 476 | 213 / 369 | 186 / 310 | 227 / 360 | 284 / 390 | 201 / 337 | 167 / 269 | 523 / 888 |
| json 14KB @1k/s → model | 243 / 385 | 330 / 609 | 223 / 408 | 294 / 420 | 222 / 352 | 227 / 364 | 266 / 442 | 303 / 478 | 222 / 401 | 218 / 346 | 550 / 917 |
| json 60KB @500/s | 429 / 558 | 562 / 1129 | 403 / 736 | 495 / 984 | 396 / 739 | 319 / 528 | 344 / 750 | 482 / 966 | 377 / 728 | 236 / 416 | 1020 / 1246 |
| json 60KB @500/s → model | 508 / 960 | 612 / 1263 | 429 / 549 | 547 / 683 | 444 / 874 | 434 / 785 | 468 / 973 | 502 / 1030 | 440 / 840 | 349 / 465 | 1138 / 1418 |
| json 60KB unicode @500/s → model | 738 / 1445 | 830 / 967 | 449 / 857 | 512 / 962 | 447 / 844 | 422 / 527 | 459 / 839 | 509 / 948 | 449 / 857 | 383 / 664 | 1012 / 2225 |
| binary 60KB @500/s | 144 / 246 | 165 / 293 | 146 / 247 | 150 / 252 | 149 / 250 | 137 / 236 | 150 / 253 | 161 / 280 | 140 / 234 | 150 / 269 | 130 / 221 |
| wss json 14KB @1k/s → model | 893 / 2334 | 359 / 740 | 1422 / 2273 | 357 / 730 | 338 / 604 | 1450 / 2298 | 337 / 631 | 336 / 696 | 241 / 430 | 244 / 370 | 576 / 975 |
| json 1MB @20/s → model (stress) | 7679 / 8682 | 9545 / 16023 | 6580 / 7936 | 8339 / 14697 | 6461 / 7936 | 6108 / 6703 | 6513 / 7413 | 7003 / 9084 | 6439 / 7528 | 3803 / 4837 | 22728 / 26131 |

### Stream — process CPU µs per message (all threads, probe excluded)

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json ~50B @20k/s | 0.9 | 1.4 | 0.9 | 1.2 | 1.4 | 0.8 | 1.5 | 1.2 | 1.1 | 1.2 | 1.6 |
| json 1KB @5k/s | 8.2 | 11.2 | 7.0 | 10.3 | 8.8 | 6.3 | 8.1 | 10.1 | 7.5 | 5.4 | 21.7 |
| json 1KB @5k/s → model | 10.1 | 13.7 | 8.6 | 12.1 | 9.4 | 8.4 | 9.5 | 10.4 | 8.4 | 6.9 | 22.0 |
| json 14KB @1k/s | 91.1 | 118.5 | 76.0 | 104.7 | 75.8 | 61.9 | 67.5 | 102.4 | 73.9 | 36.0 | 224.3 |
| json 14KB @1k/s → model | 104.6 | 131.3 | 88.5 | 116.1 | 88.1 | 91.3 | 93.5 | 101.2 | 83.9 | 62.7 | 246.3 |
| json 60KB @500/s | 331.7 | 449.3 | 274.2 | 403.7 | 273.6 | 268.4 | 279.5 | 379.6 | 271.5 | 119.7 | 939.9 |
| json 60KB @500/s → model | 389.7 | 514.6 | 326.1 | 449.3 | 333.3 | 393.0 | 410.6 | 378.5 | 322.4 | 238.8 | 1059.6 |
| json 60KB unicode @500/s → model | 627.0 | 739.0 | 329.2 | 430.6 | 333.1 | 319.6 | 330.9 | 370.5 | 331.0 | 249.5 | 935.7 |
| binary 60KB @500/s | 27.0 | 37.3 | 27.0 | 35.5 | 38.1 | 27.0 | 35.4 | 36.9 | 25.2 | 25.5 | 23.7 |
| wss json 14KB @1k/s → model | 161.5 | 229.4 | 143.3 | 215.8 | 193.4 | 145.0 | 194.4 | 199.4 | 88.9 | 67.9 | 253.1 |
| json 1MB @20/s → model (stress) | 8403.1 | 10427.5 | 6866.2 | 9064.2 | 6615.4 | 7441.0 | 7652.5 | 6980.1 | 6118.5 | 3931.1 | 23795.8 |

### Flood — throughput MB/s (higher is better)

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB | 181.1 | 60.6 | 222.2 | 58.2 | 210.2 | 288.4 | 16.2 | 119.2 | 224.7 | 448.3 | 69.6 |
| json 14KB → model | 164.2 | 59.1 | 195.2 | 62.9 | 192.6 | 196.8 | 201.6 | 126.7 | 197.2 | 462.4 | 56.0 |
| json 60KB | 195.8 | 66.9 | 239.7 | 68.3 | 253.7 | 293.0 | 263.4 | 127.4 | 249.5 | 685.0 | 53.2 |
| json 60KB → model | 167.2 | 62.5 | 196.9 | 64.1 | 202.2 | 186.0 | 286.8 | 128.9 | 202.6 | 482.5 | 51.6 |
| binary 60KB | 6490.4 | 3267.0 | 6560.0 | 3119.3 | 3111.6 | 6477.0 | 3263.9 | 3385.0 | 7331.7 | 7455.5 | 7463.6 |
| wss json 14KB → model | 96.7 | 82.9 | 109.1 | 90.6 | 159.6 | 108.8 | 114.7 | 93.4 | 197.0 | 457.7 | 56.8 |
| wss binary 60KB | 229.4 | 190.4 | 223.8 | 193.6 | 191.7 | 230.6 | 192.6 | 193.7 | 3007.2 | 2983.2 | 3001.0 |

### Flood — process CPU µs per message

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB | 6.0 | 27.4 | 4.9 | 29.4 | 5.8 | 3.8 | 70.2 | 11.7 | 5.7 | 3.0 | 18.7 |
| json 14KB → model | 90.0 | 423.4 | 76.1 | 406.4 | 79.4 | 75.0 | 109.7 | 150.3 | 79.3 | 63.8 | 375.5 |
| json 60KB | 325.9 | 1488.2 | 267.8 | 1510.3 | 259.1 | 256.9 | 280.5 | 650.6 | 268.8 | 107.1 | 1776.9 |
| json 60KB → model | 379.7 | 1685.5 | 324.4 | 1684.9 | 321.4 | 385.7 | 346.1 | 637.2 | 325.5 | 240.3 | 2028.8 |
| binary 60KB | 13.9 | 29.0 | 14.0 | 29.5 | 29.6 | 14.2 | 28.2 | 27.3 | 9.2 | 9.5 | 9.2 |
| wss json 14KB → model | 156.7 | 215.1 | 139.7 | 200.4 | 183.6 | 139.6 | 184.1 | 177.1 | 83.5 | 67.3 | 365.7 |
| wss binary 60KB | 289.1 | 389.0 | 294.6 | 385.6 | 387.5 | 287.3 | 387.3 | 385.8 | 22.9 | 22.8 | 23.1 |

### Flood — peak RSS, MB

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB | 59.3 | 338.1 | 54.5 | 312.3 | 430.6 | 54.4 | 111.1 | 132.4 | 370.8 | 32.9 | 35.9 |
| json 14KB → model | 75.5 | 380.5 | 70.1 | 332.5 | 605.2 | 67.1 | 102.7 | 161.0 | 578.4 | 733.6 | 145.8 |
| json 60KB | 73.8 | 339.3 | 63.0 | 353.4 | 662.7 | 50.1 | 63.6 | 162.7 | 545.7 | 156.9 | 844.5 |
| json 60KB → model | 66.9 | 376.6 | 72.2 | 392.5 | 636.9 | 50.3 | 205.7 | 123.7 | 526.5 | 667.3 | 888.6 |
| binary 60KB | 105.5 | 96.8 | 99.7 | 88.9 | 117.4 | 101.0 | 80.8 | 85.1 | 69.3 | 69.6 | 85.5 |
| wss json 14KB → model | 21.7 | 28.1 | 28.3 | 28.2 | 94.3 | 28.1 | 22.6 | 28.1 | 519.3 | 421.5 | 135.5 |
| wss binary 60KB | 22.7 | 24.4 | 22.6 | 23.9 | 23.7 | 22.8 | 23.9 | 24.0 | 64.5 | 62.0 | 58.4 |

### Ping-pong — msg/s (MB/s), higher is better

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB | 19410 (20.8) | 16823 (18.0) | 22768 (24.4) | 18975 (20.3) | 19648 (21.0) | 23414 (25.1) | 20022 (21.4) | 19179 (20.5) | 21245 (22.7) | 23010 (24.6) | 17473 (18.7) |
| json 14KB | 3408 (48.8) | 2834 (40.6) | 5100 (73.1) | 3648 (52.3) | 3965 (56.8) | 5491 (78.7) | 4151 (59.5) | 3520 (50.4) | 4905 (70.3) | 6353 (91.0) | 2789 (40.0) |
| json 14KB → model | 3198 (45.8) | 2669 (38.2) | 4349 (62.3) | 3332 (47.7) | 3731 (53.5) | 4552 (65.2) | 3625 (51.9) | 3536 (50.7) | 4272 (61.2) | 5480 (78.5) | 2649 (38.0) |
| json 30KB | 1847 (56.9) | 1523 (46.9) | 2528 (77.9) | 1954 (60.2) | 2175 (67.0) | 2770 (85.4) | 2342 (72.2) | 1986 (61.2) | 2523 (77.7) | 3226 (99.4) | 1453 (44.8) |
| json 60KB | 995 (61.1) | 815 (50.1) | 1359 (83.5) | 1052 (64.6) | 1187 (73.0) | 1432 (88.0) | 1234 (75.9) | 1064 (65.4) | 1372 (84.3) | 1785 (109.7) | 724 (44.5) |
| json 60KB → model | 930 (57.2) | 766 (47.1) | 1246 (76.6) | 954 (58.6) | 1100 (67.6) | 1200 (73.8) | 1040 (63.9) | 1037 (63.7) | 1230 (75.6) | 1443 (88.7) | 673 (41.4) |
| binary 60KB | 17353 (1066.2) | 14415 (885.7) | 16201 (995.4) | 14084 (865.3) | 13959 (857.6) | 16156 (992.6) | 14161 (870.0) | 14056 (863.6) | 17141 (1053.1) | 17157 (1054.1) | 17367 (1067.0) |
| wss json 14KB → model | 2412 (34.6) | 1930 (27.7) | 2987 (42.8) | 2388 (34.2) | 2592 (37.1) | 3015 (43.2) | 2586 (37.1) | 2565 (36.7) | 4005 (57.4) | 4909 (70.3) | 2498 (35.8) |

### Ping-pong — RTT p50 / p99, µs

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB | 50 / 77 | 58 / 93 | 43 / 68 | 52 / 83 | 50 / 83 | 42 / 69 | 49 / 78 | 52 / 80 | 46 / 72 | 43 / 61 | 56 / 77 |
| json 14KB | 290 / 347 | 346 / 440 | 191 / 247 | 269 / 340 | 244 / 304 | 178 / 226 | 232 / 312 | 275 / 352 | 195 / 278 | 155 / 198 | 355 / 419 |
| json 14KB → model | 309 / 382 | 369 / 467 | 225 / 304 | 297 / 365 | 263 / 341 | 217 / 289 | 268 / 335 | 273 / 358 | 229 / 293 | 179 / 224 | 372 / 454 |
| json 30KB | 535 / 641 | 646 / 809 | 391 / 478 | 508 / 717 | 450 / 603 | 355 / 424 | 422 / 502 | 499 / 589 | 390 / 484 | 303 / 384 | 685 / 791 |
| json 60KB | 996 / 1155 | 1221 / 1434 | 732 / 899 | 946 / 1127 | 833 / 992 | 683 / 911 | 786 / 1089 | 933 / 1101 | 724 / 879 | 557 / 670 | 1375 / 1626 |
| json 60KB → model | 1064 / 1275 | 1290 / 1623 | 792 / 976 | 1032 / 1414 | 902 / 1036 | 814 / 1007 | 930 / 1234 | 954 / 1086 | 799 / 948 | 690 / 797 | 1475 / 1753 |
| binary 60KB | 56 / 91 | 65 / 112 | 60 / 90 | 69 / 103 | 69 / 112 | 60 / 91 | 69 / 104 | 69 / 107 | 57 / 88 | 56 / 94 | 56 / 86 |
| wss json 14KB → model | 410 / 500 | 513 / 614 | 331 / 414 | 411 / 491 | 382 / 479 | 327 / 397 | 383 / 460 | 383 / 490 | 246 / 310 | 200 / 250 | 398 / 477 |

### Ping-pong — process CPU µs per message

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB | 41.3 | 51.6 | 31.3 | 43.9 | 43.4 | 30.2 | 41.7 | 43.4 | 40.6 | 36.4 | 50.4 |
| json 14KB | 255.1 | 319.5 | 183.5 | 238.3 | 216.5 | 169.9 | 206.8 | 248.6 | 192.5 | 149.5 | 322.1 |
| json 14KB → model | 274.4 | 340.5 | 197.5 | 262.1 | 232.8 | 196.9 | 239.9 | 249.1 | 198.6 | 175.1 | 341.9 |
| json 30KB | 499.7 | 624.5 | 353.7 | 479.6 | 427.8 | 319.9 | 390.5 | 465.3 | 358.6 | 273.8 | 636.9 |
| json 60KB | 952.0 | 1181.0 | 689.1 | 914.2 | 803.2 | 697.4 | 823.9 | 899.4 | 684.3 | 520.5 | 1317.3 |
| json 60KB → model | 1018.5 | 1260.3 | 759.3 | 1005.6 | 868.6 | 840.1 | 976.5 | 925.1 | 767.6 | 644.7 | 1424.9 |
| binary 60KB | 47.6 | 63.6 | 48.1 | 62.2 | 62.8 | 48.3 | 61.9 | 62.4 | 43.7 | 43.6 | 43.0 |
| wss json 14KB → model | 400.0 | 545.5 | 320.8 | 433.7 | 404.1 | 319.0 | 400.8 | 405.6 | 212.2 | 188.3 | 358.4 |

### Echo — RTT p50 / p99, µs

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB @1k/s | 411 / 1279 | 541 / 1993 | 343 / 1689 | 489 / 3281 | 413 / 1173 | 338 / 1292 | 456 / 3264 | 470 / 1917 | 377 / 1585 | 333 / 1079 | 445 / 1385 |
| binary 60KB @200/s | 544 / 1472 | 580 / 1581 | 467 / 1199 | 676 / 1429 | 601 / 1583 | 587 / 1660 | 641 / 1351 | 641 / 1317 | 592 / 1788 | 607 / 1680 | 594 / 1548 |

### Echo — process CPU µs per message

| case | a | b | c | c-worker | c-worker-raw | c-tape | c-worker-tape | c-worker-model | d-bytes | d-tape | d-cobject |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| json 1KB @1k/s | 314.8 | 380.0 | 251.3 | 315.3 | 290.9 | 244.3 | 318.5 | 323.8 | 290.0 | 270.7 | 341.0 |
| binary 60KB @200/s | 496.3 | 590.2 | 401.9 | 613.0 | 533.9 | 507.5 | 569.7 | 575.5 | 518.0 | 500.5 | 499.9 |

## Mobile devices (wss, embedded server)

Date: 2026-11-12. Flutter 3.47.9 (Dart 3.13.7), release builds. The server
runs inside the app process on 127.0.0.1 with 2 threads, and its CPU time is
excluded. Every variant runs 2 times; the tables show the median, which with
2 runs is the higher one. Raw runs are in `results/`.

- **Android:** Samsung Galaxy S25 Ultra (SM-S938B, Snapdragon 8 Elite, SM8750).
- **iOS:** iPad Pro 11" 4th gen (iPad14,3, Apple M2).

Caveats:

- The **main isolate µs** metric has a noise floor of about ±1% busy time.
  At 100 msg/s that is about ±100 µs/msg, so negative or near-zero values
  mean "below resolution". They show up for the worker and `d-tape` variants.
- **Peak RSS** is not meaningful, because all cases share one process.
- **Android anomaly:** `d-tape` ping-pong at 14 KB is slow (846–924 msg/s)
  in both runs, and so is the embedded server's CPU time. This is probably
  DVFS or core placement when main is almost idle. Not investigated.

### Android — Galaxy S25 Ultra

#### Stream — main isolate µs per message (lower is better)

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 31.83 | 33.70 | 12.07 | 11.65 | 8.88 | 7.60 |
| wss json 14KB @500/s → model | 251.32 | 243.15 | 50.64 | 41.32 | 96.28 | 33.72 |
| wss json 60KB @100/s → model | 949.78 | 895.58 | -103.43 | 75.51 | 323.44 | 98.76 |
| wss binary 60KB @200/s | 562.40 | 560.03 | 140.34 | 122.19 | 8.71 | 11.02 |

#### Stream — longest main isolate stall, µs

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 465 | 316 | 412 | 443 | 288 | 260 |
| wss json 14KB @500/s → model | 627 | 440 | 465 | 426 | 476 | 323 |
| wss json 60KB @100/s → model | 1311 | 1257 | 971 | 1124 | 1365 | 997 |
| wss binary 60KB @200/s | 324 | 367 | 329 | 498 | 1410 | 82 |

#### Stream — delivery latency p50 / p99, µs

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 559 / 880 | 554 / 797 | 491 / 978 | 484 / 899 | 171 / 359 | 205 / 375 |
| wss json 14KB @500/s → model | 1143 / 2131 | 1138 / 2306 | 1099 / 1616 | 1058 / 2287 | 289 / 610 | 344 / 552 |
| wss json 60KB @100/s → model | 4146 / 4567 | 4079 / 4542 | 4493 / 6827 | 4152 / 5259 | 778 / 1218 | 964 / 1408 |
| wss binary 60KB @200/s | 3615 / 3952 | 3622 / 4060 | 1951 / 2641 | 1982 / 2846 | 350 / 689 | 349 / 645 |

#### Stream — process CPU µs per message (all threads, probe excluded)

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 94.7 | 95.3 | 182.4 | 186.6 | 36.9 | 52.4 |
| wss json 14KB @500/s → model | 632.6 | 612.7 | 1281.4 | 1168.5 | 246.8 | 308.4 |
| wss json 60KB @100/s → model | 2360.4 | 2325.8 | 5511.5 | 4887.7 | 1048.5 | 1067.4 |
| wss binary 60KB @200/s | 1641.5 | 1661.0 | 2601.7 | 2587.9 | 379.6 | 373.1 |

#### Flood — throughput MB/s (higher is better)

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| ws json 14KB → model | 113.3 | 117.7 | 29.6 | 59.3 | 140.3 | 255.4 |
| wss json 14KB → model | 31.0 | 33.1 | 39.7 | 40.8 | 139.3 | 268.1 |
| wss json 60KB → model | 31.1 | 33.4 | 37.7 | 41.1 | 169.1 | 330.4 |
| wss binary 60KB | 51.4 | 51.0 | 79.0 | 78.6 | 1608.1 | 1674.9 |

#### Flood — process CPU µs per message

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| ws json 14KB → model | 146.0 | 155.9 | 999.1 | 419.6 | 114.5 | 105.0 |
| wss json 14KB → model | 461.5 | 437.2 | 447.0 | 403.1 | 119.1 | 104.3 |
| wss json 60KB → model | 1967.0 | 1856.1 | 1974.8 | 1703.2 | 418.6 | 401.6 |
| wss binary 60KB | 1235.0 | 1250.9 | 995.9 | 1002.6 | 66.1 | 57.0 |

#### Flood — peak RSS, MB

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| ws json 14KB → model | 239.9 | 239.9 | 510.4 | 510.4 | 510.4 | 510.4 |
| wss json 14KB → model | 510.4 | 510.4 | 510.4 | 510.4 | 510.4 | 510.4 |
| wss json 60KB → model | 510.4 | 510.4 | 510.4 | 510.4 | 510.4 | 510.4 |
| wss binary 60KB | 510.4 | 510.4 | 510.4 | 510.4 | 510.4 | 510.4 |

#### Ping-pong — msg/s (MB/s), higher is better

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB → model | 3192 (3.4) | 3311 (3.5) | 2895 (3.1) | 2712 (2.9) | 3636 (3.9) | 2775 (3.0) |
| wss json 14KB → model | 1058 (15.2) | 1304 (18.7) | 1256 (18.0) | 1369 (19.6) | 2363 (33.9) | 924 (13.2) |
| wss json 60KB → model | 275 (16.9) | 327 (20.1) | 327 (20.1) | 335 (20.6) | 627 (38.5) | 595 (36.6) |

#### Ping-pong — RTT p50 / p99, µs

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB → model | 305 / 449 | 296 / 413 | 326 / 616 | 351 / 686 | 214 / 603 | 365 / 629 |
| wss json 14KB → model | 895 / 1592 | 750 / 1053 | 755 / 1734 | 711 / 1022 | 398 / 995 | 1078 / 2446 |
| wss json 60KB → model | 3514 / 4586 | 3022 / 3539 | 3010 / 3653 | 2927 / 3641 | 1558 / 1998 | 1614 / 2518 |

#### Ping-pong — process CPU µs per message

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB → model | 305.3 | 300.2 | 439.2 | 461.5 | 244.0 | 322.6 |
| wss json 14KB → model | 871.6 | 691.3 | 861.5 | 794.6 | 390.9 | 996.2 |
| wss json 60KB → model | 3365.6 | 2773.5 | 3265.9 | 3190.3 | 1499.6 | 1568.7 |

### iOS — iPad Pro 11" (M2)

#### Stream — main isolate µs per message (lower is better)

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 18.03 | 14.28 | -3.21 | -3.51 | 6.41 | -0.56 |
| wss json 14KB @500/s → model | 134.22 | 125.93 | 19.99 | -21.93 | 62.13 | 2.82 |
| wss json 60KB @100/s → model | 544.69 | 531.82 | -18.90 | -72.50 | 200.28 | -6.81 |
| wss binary 60KB @200/s | 201.97 | 179.38 | -40.21 | -63.58 | -53.74 | -58.77 |

#### Stream — longest main isolate stall, µs

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 260 | 149 | 53 | 107 | 135 | 132 |
| wss json 14KB @500/s → model | 373 | 366 | 270 | 312 | 463 | 347 |
| wss json 60KB @100/s → model | 1262 | 1200 | 903 | 941 | 1282 | 1137 |
| wss binary 60KB @200/s | 243 | 545 | 266 | 79 | 134 | 74 |

#### Stream — delivery latency p50 / p99, µs

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 479 / 551 | 473 / 532 | 155 / 237 | 153 / 258 | 107 / 171 | 105 / 172 |
| wss json 14KB @500/s → model | 918 / 1678 | 887 / 1758 | 281 / 558 | 268 / 480 | 183 / 255 | 176 / 292 |
| wss json 60KB @100/s → model | 3391 / 3672 | 3343 / 3571 | 949 / 1125 | 897 / 1154 | 509 / 782 | 404 / 561 |
| wss binary 60KB @200/s | 2972 / 3093 | 2968 / 3130 | 441 / 573 | 446 / 610 | 148 / 222 | 143 / 209 |

#### Stream — process CPU µs per message (all threads, probe excluded)

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB @2k/s → model | 69.9 | 66.4 | 69.7 | 66.3 | 47.3 | 46.5 |
| wss json 14KB @500/s → model | 360.5 | 340.8 | 365.5 | 365.1 | 243.6 | 222.9 |
| wss json 60KB @100/s → model | 1541.8 | 1506.7 | 1655.0 | 1619.7 | 1100.2 | 1011.5 |
| wss binary 60KB @200/s | 776.4 | 769.7 | 785.3 | 786.3 | 410.4 | 401.1 |

#### Flood — throughput MB/s (higher is better)

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| ws json 14KB → model | 135.5 | 155.7 | 54.9 | 103.0 | 167.5 | 378.8 |
| wss json 14KB → model | 53.5 | 58.3 | 81.8 | 82.7 | 166.4 | 382.5 |
| wss json 60KB → model | 37.9 | 40.0 | 81.1 | 82.7 | 175.7 | 416.8 |
| wss binary 60KB | 92.9 | 93.5 | 167.7 | 167.0 | 1715.7 | 1723.8 |

#### Flood — process CPU µs per message

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| ws json 14KB → model | 113.1 | 100.1 | 441.7 | 194.9 | 97.5 | 78.0 |
| wss json 14KB → model | 297.0 | 278.0 | 236.5 | 214.3 | 103.4 | 83.5 |
| wss json 60KB → model | 1245.1 | 1163.4 | 953.6 | 854.4 | 393.8 | 324.3 |
| wss binary 60KB | 803.8 | 801.3 | 485.2 | 481.7 | 41.6 | 41.4 |

#### Flood — peak RSS, MB

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| ws json 14KB → model | 172.4 | 172.4 | 396.4 | 396.4 | 396.4 | 602.1 |
| wss json 14KB → model | 396.4 | 396.4 | 602.1 | 396.4 | 602.1 | 396.4 |
| wss json 60KB → model | 396.4 | 602.1 | 396.4 | 602.1 | 396.4 | 396.4 |
| wss binary 60KB | 396.4 | 396.4 | 396.4 | 396.4 | 396.4 | 602.1 |

#### Ping-pong — msg/s (MB/s), higher is better

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB → model | 4934 (5.3) | 5151 (5.5) | 7555 (8.1) | 7625 (8.2) | 12869 (13.8) | 12964 (13.9) |
| wss json 14KB → model | 1278 (18.3) | 1451 (20.8) | 2303 (33.0) | 2400 (34.4) | 2460 (35.2) | 3819 (54.7) |
| wss json 60KB → model | 333 (20.5) | 387 (23.8) | 548 (33.7) | 580 (35.6) | 933 (57.3) | 1059 (65.1) |

#### Ping-pong — RTT p50 / p99, µs

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB → model | 198 / 359 | 190 / 384 | 128 / 285 | 127 / 306 | 73 / 215 | 72 / 210 |
| wss json 14KB → model | 754 / 1150 | 655 / 1135 | 422 / 763 | 403 / 816 | 389 / 700 | 244 / 628 |
| wss json 60KB → model | 2966 / 3687 | 2523 / 3905 | 1785 / 2368 | 1702 / 2093 | 1052 / 1612 | 929 / 1690 |

#### Ping-pong — process CPU µs per message

| case | a | c | c-worker | c-worker-model | d-bytes | d-tape |
|---|---:|---:|---:|---:|---:|---:|
| wss json 1KB → model | 242.7 | 235.0 | 154.1 | 154.2 | 71.1 | 68.7 |
| wss json 14KB → model | 662.7 | 574.7 | 473.0 | 456.3 | 283.5 | 234.5 |
| wss json 60KB → model | 2569.6 | 2167.1 | 1881.5 | 1751.3 | 930.1 | 791.6 |
