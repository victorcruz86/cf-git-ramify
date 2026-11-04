# Log graph: measurements

Repository `~/rama-medida/git` (git/git): 85,936 commits and 1,022 references. Release binary with the project profile (`opt-level = "s"`, LTO), on an arm64 Mac. `open_repo` and `rows_in` are called directly, without Tauri: it does not include the round trip between processes or the drawing. Three runs.

## Targets

| Measurement | Target | Result |
|---|---|---|
| `log_open` | < 2 s | 0.87 / 0.89 / 1.06 s |
| `log_rows` of 50 rows, worst point of the log | < 5 ms | 0.19 / 0.22 / 0.23 ms |

`log_rows` was measured from 627 points of the log, one every 137 rows, crossing the checkpoints.

## Detail

| Measurement | Median | p99 | Maximum |
|---|---|---|---|
| `log_rows`, 50 rows | 0.10 ms | 0.19 ms | 0.23 ms |
| `log_rows`, 200 rows, what each `RowCache` page asks for | 0.29 ms | 0.44 ms | 0.48 ms |
| Graph only, 50 rows, Long Edges off | 0.10 ms | 0.18 ms | 0.21 ms |
| Graph only, 50 rows, Long Edges on | 0.54 ms | 1.17 ms | 1.50 ms |
| JSON of 200 rows, up to 194 KB | 0.28 ms | | 0.40 ms |

`View::build` takes 22 ms of the `log_open`. Computing all the rows at once, 1,000 at a time, takes 0.10 s with Long Edges off and 0.85 s on.

## Memory

RSS increase of the process after `open_repo`, measured with `ps` before and after, with the same program on both commits:

| | RSS delta |
|---|---|
| Before, with lanes (510b705) | 105.3 / 105.4 / 105.7 MB |
| Now | 40.5 / 40.5 / 41.9 MB |

It is not the memory of the whole application. The drop comes from not storing each row's data: the lanes stored all the segments of each row, and now they are computed when requested.

The checkpoints take very little: 86 points with 11,380 edges in total, 296 at most in one, 0.13 MB.

## Graph shape

| | Long Edges off | Long Edges on |
|---|---|---|
| `recommendedWidth` | 8 | 124 |
| Columns of the widest row | 111 | 249 |
| Segments per row, mean | 7.0 | 164.3 |
| Segments per row, maximum | 113 | 499 |

No row of git/git runs into the comparator's lack of transitivity (see the comment of `merge_sort` in `src-tauri/src/log/print.rs`).

## Order and collapse

Same repository and profile. `set_view` and `graph_click` are called directly, without Tauri, on the already open `OpenRepo`. Three runs. The target for changing the order is less than 3 s.

| Change | Time |
|---|---|
| Off to Standard, first time, with the BEK sort | 22.8 / 22.9 / 23.4 ms |
| Off to Standard, with BEK already computed | 16.9 / 17.0 / 17.0 ms |
| Standard to Off | 18.7 / 18.7 / 19.1 ms |
| Standard to Linear, with all the merges folded | 106 / 108 / 108 ms |
| Linear to Off | 19.8 / 19.8 / 20.3 ms |
| `Collapse Merges` | 106 / 109 / 111 ms |
| `Expand Merges` | 23.1 / 23.1 / 23.2 ms |
| `Collapse Linear Branches` in Off | 64.3 / 64.6 / 64.7 ms |
| `Collapse Linear Branches` in Standard | 61.3 / 61.8 / 63.1 ms |
| `Expand Linear Branches` in Off | 18.9 / 18.9 / 19.0 ms |

With the linear branches collapsed, 49,844 rows of 85,936 remain. The BEK sort itself takes about 6 ms; the rest of each change is recomputing the checkpoints and the recommended width.

A click that changes the graph costs about 20 ms in Off and Standard, and between 10 and 24 ms in Linear. Almost all of it is recompiling the graph and the checkpoints. It was tried on three rows of each order.

`log_rows` of 50 rows, worst point of 627, three runs: 0.17 / 0.17 / 0.24 ms in Standard and 0.08 / 0.08 / 0.12 ms in Linear.

| RSS increase | |
|---|---|
| After `open_repo` | 42.1 / 42.1 / 42.9 MB |
| Additionally, after switching to Standard | 4.6 / 4.6 / 6.1 MB |
| Additionally, after switching to Linear | 13.2 / 13.3 / 14.3 MB |
| Additionally, after going back to Off | 13.2 / 13.3 / 14.3 MB |

The BEK graph stays in memory for the next time, like `myBekIntMap`. What Linear takes is not returned to the system when going back to Off.

## Fidelity of order and collapse

IntelliJ's real Java files (`BekBranch*`, `LinearBekGraph*`, `EdgeStorage*`, `FragmentGenerator`, `LinearFragmentGenerator`, `CollapsedGraph`, `CollapsedActionManager`) were compiled with minimal adapters for everything else, and their outputs were compared with those of this code, byte by byte:

| Input | Compared | Result |
|---|---|---|
| git/git | BEK order, folded Linear and `Collapse Linear Branches` in Off and in Standard | Identical |
| 4,000 random graphs | The same, with random branches and dates | Identical |
| 6,000 random graphs | 240,000 clicks on nodes in Off and Standard, starting from everything expanded | Identical |

The `COLLAPSE_ALL` loop and `LinearBekController.kt` were not compiled: the first was copied into the comparison program, and the second is Kotlin and there is no compiler here. Clicks in Linear order are covered only by the tests. In addition, 600,000 random graphs always give a permutation with the children before the parents.
