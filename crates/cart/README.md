# jobniac-cart

A CART (Classification and Regression Trees, classification only) decision-tree library in pure
Rust with no dependencies. It is generic: it works on numeric feature rows and class labels and
knows nothing about what they mean.

- `Dataset`: named features and classes, `f64` rows, a label and an optional weight per row;
  stratified train/test split and k-fold partitions driven by a built-in SplitMix64.
- `train(&Dataset, &TrainConfig) -> Tree`: greedy binary splits `x <= threshold` (Gini or entropy),
  with `max_depth`, `min_samples_split`, `min_samples_leaf`, `min_impurity_decrease` and class
  weights.
- `Tree`: `predict`, `predict_proba`, `depth`, `leaf_count`, `feature_importances`.
- Pruning: `cost_complexity_path`, `prune` (impurity-based weakest-link pruning, as scikit-learn's
  `ccp_alpha`) and `select_alpha` (stratified k-fold choice of alpha).
- `ConfusionMatrix`: accuracy, per-class precision, recall and F1, macro-F1, plain-text table.
- `Tree::to_text` / `Tree::from_text`: exact, versioned model format.

## Training rules

For every node and feature the rows are sorted by `f64::total_cmp` (ties by row index) and swept
with cumulative weighted class counts. Only thresholds between distinct consecutive values that keep
`min_samples_leaf` rows on each side are tried; the threshold is `a + (b - a) / 2` (or `a` if
rounding makes it equal `b`). The largest weighted impurity decrease wins; exact ties go to the lower
feature index, then the lower threshold. A node stops on `max_depth`, too few rows, purity, or a
decrease `W_t / W_root * gain` below `min_impurity_decrease`. A zero-gain split is allowed when the
threshold is 0 (needed for XOR-like data). Cost is about `O(depth * features * rows * log rows)`.

## Pruning cost

One weakest-link sequence costs `O(nodes^2)`. `select_alpha` computes it once per fold tree and
evaluates every candidate alpha from a prefix of that sequence (the same result as pruning each
alpha separately), so it costs `k` trainings plus `k * O(nodes^2)` plus
`k * candidates * O(nodes + rows)`. An ignored timing test covers 20,000 noisy rows.

## Determinism

The same input gives bit-identical trees and text on every platform: no threads, no hash maps,
fixed summation order, SplitMix64 for all randomness, and an entropy `log2` built from `+ - * /`
only (the platform `log2` is not used). Gini needs nothing beyond IEEE arithmetic.

## Model text format (version 1)

```text
jobniac-cart 1
features <n> <name>...
classes <n> <name>...
criterion gini|entropy
max_depth none|<n>
min_samples_split <n>
min_samples_leaf <n>
min_impurity_decrease <hex16>
class_weights none|<n> <hex16>...
nodes <n>
L <samples> <count hex16>...                                  leaf
S <samples> <feature> <threshold hex16> <left> <right> <count hex16>...   split
end
```

Lines end with `\n`; fields are separated by one space; integers are plain decimal. Every `f64` is
the 16 lower-case hex digits of its `to_bits` pattern, so `from_text(to_text(t)) == t` exactly. Names
match `[A-Za-z0-9_.-]{1,128}`. Node 0 is the root and every child index is larger than its parent's.

`from_text` treats its input as untrusted and never panics. It rejects a wrong header or version,
truncation, counts that disagree with the lists they announce, child indices that are out of range,
not increasing, shared or missing a parent, child sample counts that do not add up, NaN or infinite
values, non-canonical numbers, an invalid config, and anything after `end`. Limits: 64 MiB of text,
1,000,000 nodes, 4,096 features, 1,024 classes.

## Tests and the golden files

Tests combine hand-computed values (impurities, a best split, the alpha sequence, a confusion
matrix) with golden files: `tests/golden/synthetic_tree.txt` (Gini) and
`tests/golden/synthetic_tree_entropy.txt` are the trees trained on a fixed 300-row synthetic dataset
and must match byte for byte. After an intentional change of the algorithm or the format, regenerate
them and review the diff:

```text
JOBNIAC_CART_REGEN=1 cargo test -p jobniac-cart --test golden
```

A timing check on 20,000 rows by 30 features is ignored by default:

```text
cargo test -p jobniac-cart --release --test perf -- --ignored --nocapture
```
