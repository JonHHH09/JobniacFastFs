//! The versioned, exact model text format (`jobniac-cart 1`).
//!
//! ```text
//! jobniac-cart 1
//! features <n> <name>...
//! classes <n> <name>...
//! criterion gini|entropy
//! max_depth none|<n>
//! min_samples_split <n>
//! min_samples_leaf <n>
//! min_impurity_decrease <hex16>
//! class_weights none|<n> <hex16>...
//! nodes <n>
//! L <samples> <count hex16>...                                      (leaf, one count per class)
//! S <samples> <feature> <threshold hex16> <left> <right> <count hex16>...   (split)
//! end
//! ```
//!
//! Lines end with `\n` (also the last one), fields are separated by one space, integers are plain
//! decimal without sign or leading zeros, and every `f64` is the 16 lower-case hex digits of its
//! `to_bits` pattern, so a round trip is bit-exact. The parser treats its input as untrusted and
//! never panics; it enforces [`MAX_MODEL_BYTES`], [`MAX_NODES`], [`MAX_FEATURES`] and
//! [`MAX_CLASSES`].

use std::fmt::Write as _;

use crate::config::{Criterion, TrainConfig};
use crate::dataset::{MAX_CLASSES, MAX_FEATURES, validate_names};
use crate::error::CartError;
use crate::tree::{MAX_NODES, Node, Split, Tree, validate_nodes};

/// The first line of every model file.
pub const HEADER: &str = "jobniac-cart 1";
/// Largest model text, in bytes, that [`Tree::from_text`] accepts.
pub const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;

fn hex(value: f64) -> String {
    format!("{:016x}", value.to_bits())
}

impl Tree {
    /// Serialises the tree in the model text format. The output is a pure function of the tree.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{HEADER}");
        let _ = writeln!(
            out,
            "features {} {}",
            self.feature_names.len(),
            self.feature_names.join(" ")
        );
        let _ = writeln!(
            out,
            "classes {} {}",
            self.class_names.len(),
            self.class_names.join(" ")
        );
        let c = &self.config;
        let _ = writeln!(out, "criterion {}", c.criterion.as_str());
        match c.max_depth {
            Some(d) => {
                let _ = writeln!(out, "max_depth {d}");
            }
            None => out.push_str("max_depth none\n"),
        }
        let _ = writeln!(out, "min_samples_split {}", c.min_samples_split);
        let _ = writeln!(out, "min_samples_leaf {}", c.min_samples_leaf);
        let _ = writeln!(
            out,
            "min_impurity_decrease {}",
            hex(c.min_impurity_decrease)
        );
        match &c.class_weights {
            Some(w) => {
                let hexes: Vec<String> = w.iter().map(|&x| hex(x)).collect();
                let _ = writeln!(out, "class_weights {} {}", w.len(), hexes.join(" "));
            }
            None => out.push_str("class_weights none\n"),
        }
        let _ = writeln!(out, "nodes {}", self.nodes.len());
        for node in &self.nodes {
            match &node.split {
                None => {
                    let _ = write!(out, "L {}", node.samples);
                }
                Some(s) => {
                    let _ = write!(
                        out,
                        "S {} {} {} {} {}",
                        node.samples,
                        s.feature,
                        hex(s.threshold),
                        s.left,
                        s.right
                    );
                }
            }
            for &count in &node.counts {
                let _ = write!(out, " {}", hex(count));
            }
            out.push('\n');
        }
        out.push_str("end\n");
        out
    }

    /// Parses model text produced by [`Tree::to_text`].
    ///
    /// Rejects a wrong header or version, truncated input, counts that do not match the lists they
    /// announce, out-of-range or non-increasing child indices, nodes with more or fewer than one
    /// parent, child sample counts that do not add up, NaN or infinite numbers, malformed numbers,
    /// invalid or repeated names, an invalid config, text after `end`, and input larger than
    /// [`MAX_MODEL_BYTES`] or declaring more than [`MAX_NODES`] nodes, [`MAX_FEATURES`] features or
    /// [`MAX_CLASSES`] classes. Child class counts are only checked for being finite and not
    /// negative (training sums them in a different order than a parent, so exact equality with
    /// the parent is not guaranteed); child sample counts must add up exactly.
    ///
    /// # Errors
    /// [`CartError::InvalidModel`] (with the 1-based line number where known) for any of the above.
    pub fn from_text(text: &str) -> Result<Tree, CartError> {
        Parser::new(text)?.parse()
    }
}

struct Parser<'a> {
    lines: std::str::Split<'a, char>,
    line_no: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Result<Self, CartError> {
        if text.len() > MAX_MODEL_BYTES {
            return Err(err(0, format!("input larger than {MAX_MODEL_BYTES} bytes")));
        }
        Ok(Self {
            lines: text.split('\n'),
            line_no: 0,
        })
    }

    fn next_line(&mut self) -> Result<&'a str, CartError> {
        self.line_no += 1;
        self.lines
            .next()
            .ok_or_else(|| err(self.line_no, "unexpected end of input".to_owned()))
    }

    fn fail<T>(&self, reason: &str) -> Result<T, CartError> {
        Err(err(self.line_no, reason.to_owned()))
    }

    /// Reads a line and splits it into fields; the first field must be `keyword`.
    fn keyed_line(&mut self, keyword: &str) -> Result<Vec<&'a str>, CartError> {
        let line = self.next_line()?;
        let Some(fields) = bounded_fields(line, MAX_FEATURES + 2) else {
            return self.fail("too many fields on one line");
        };
        if fields.first() != Some(&keyword) {
            return self.fail(&format!("expected a `{keyword}` line"));
        }
        Ok(fields)
    }

    fn parse(mut self) -> Result<Tree, CartError> {
        let header = self.next_line()?;
        if header != HEADER {
            return if header.starts_with("jobniac-cart ") {
                self.fail("unsupported model version")
            } else {
                self.fail("not a jobniac-cart model")
            };
        }
        let feature_names =
            self.name_list("features", "feature", MAX_FEATURES, CartError::NoFeatures)?;
        let class_names = self.name_list("classes", "class", MAX_CLASSES, CartError::NoClasses)?;

        let fields = self.keyed_line("criterion")?;
        let criterion = match fields.as_slice() {
            [_, name] => Criterion::parse(name),
            _ => None,
        };
        let Some(criterion) = criterion else {
            return self.fail("unknown criterion");
        };

        let fields = self.keyed_line("max_depth")?;
        let max_depth = match fields.as_slice() {
            [_, "none"] => None,
            [_, value] => Some(self.integer(value, usize::MAX)?),
            _ => return self.fail("malformed max_depth line"),
        };
        let min_samples_split = self.single_integer("min_samples_split")?;
        let min_samples_leaf = self.single_integer("min_samples_leaf")?;

        let fields = self.keyed_line("min_impurity_decrease")?;
        let [_, value] = fields.as_slice() else {
            return self.fail("malformed min_impurity_decrease line");
        };
        let min_impurity_decrease = self.float(value)?;

        let fields = self.keyed_line("class_weights")?;
        let class_weights = match fields.as_slice() {
            [_, "none"] => None,
            [_, count, values @ ..] => {
                let n = self.integer(count, MAX_CLASSES)?;
                if n != class_names.len() || values.len() != n {
                    return self.fail("class_weights does not match the class count");
                }
                let mut weights = Vec::with_capacity(n);
                for v in values {
                    weights.push(self.float(v)?);
                }
                Some(weights)
            }
            _ => return self.fail("malformed class_weights line"),
        };
        let config = TrainConfig {
            criterion,
            max_depth,
            min_samples_split,
            min_samples_leaf,
            min_impurity_decrease,
            class_weights,
        };
        if let Err(e) = config.validate() {
            return self.fail(&e.to_string());
        }

        let fields = self.keyed_line("nodes")?;
        let [_, count] = fields.as_slice() else {
            return self.fail("malformed nodes line");
        };
        let node_count = self.integer(count, MAX_NODES)?;
        if node_count == 0 {
            return self.fail("a model needs at least one node");
        }
        let first_node_line = self.line_no + 1;
        let mut nodes = Vec::new();
        for _ in 0..node_count {
            let line = self.next_line()?;
            nodes.push(self.node(line, feature_names.len(), class_names.len())?);
        }
        if self.next_line()? != "end" {
            return self.fail("expected `end`");
        }
        // After `end\n` the split iterator must yield exactly one empty tail.
        match (self.lines.next(), self.lines.next()) {
            (Some(""), None) => {}
            _ => return err_at(self.line_no + 1, "unexpected data after `end`"),
        }
        if let Err((index, reason)) = validate_nodes(&nodes, feature_names.len(), class_names.len())
        {
            return Err(err(first_node_line + index, reason));
        }
        Ok(Tree {
            feature_names,
            class_names,
            config,
            nodes,
        })
    }

    fn name_list(
        &mut self,
        keyword: &str,
        kind: &'static str,
        max: usize,
        empty: CartError,
    ) -> Result<Vec<String>, CartError> {
        let fields = self.keyed_line(keyword)?;
        let Some(count) = fields.get(1) else {
            return self.fail("missing count");
        };
        let n = self.integer(count, max)?;
        if n == 0 {
            return Err(err(self.line_no, empty.to_string()));
        }
        if fields.len() != n + 2 {
            return self.fail("name count does not match the announced count");
        }
        let names: Vec<String> = fields[2..].iter().map(|s| (*s).to_owned()).collect();
        if let Err(e) = validate_names(kind, &names, max, empty) {
            return self.fail(&e.to_string());
        }
        Ok(names)
    }

    fn single_integer(&mut self, keyword: &str) -> Result<usize, CartError> {
        let fields = self.keyed_line(keyword)?;
        match fields.as_slice() {
            [_, value] => self.integer(value, usize::MAX),
            _ => self.fail(&format!("malformed {keyword} line")),
        }
    }

    /// Plain decimal: digits only, no leading zeros (except `0`), at most `max`.
    fn integer(&self, token: &str, max: usize) -> Result<usize, CartError> {
        let canonical = !token.is_empty()
            && token.bytes().all(|b| b.is_ascii_digit())
            && (token == "0" || !token.starts_with('0'));
        if !canonical {
            return self.fail(&format!("`{}` is not a canonical integer", clip(token)));
        }
        match token.parse::<usize>() {
            Ok(v) if v <= max => Ok(v),
            _ => self.fail(&format!("integer `{}` is out of range", clip(token))),
        }
    }

    /// Exactly 16 lower-case hex digits, a finite value.
    fn float(&self, token: &str) -> Result<f64, CartError> {
        let well_formed = token.len() == 16
            && token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !well_formed {
            return self.fail(&format!("`{}` is not a 16-digit hex float", clip(token)));
        }
        let Ok(bits) = u64::from_str_radix(token, 16) else {
            return self.fail("malformed hex float");
        };
        let value = f64::from_bits(bits);
        if !value.is_finite() {
            return self.fail("number is not finite");
        }
        Ok(value)
    }

    fn node(
        &self,
        line: &str,
        feature_count: usize,
        class_count: usize,
    ) -> Result<Node, CartError> {
        let Some(fields) = bounded_fields(line, 6 + class_count) else {
            return self.fail("too many fields on one line");
        };
        let (header_len, is_split) = match fields.first() {
            Some(&"L") => (2, false),
            Some(&"S") => (6, true),
            _ => return self.fail("a node line starts with `L` or `S`"),
        };
        if fields.len() != header_len + class_count {
            return self.fail("wrong number of fields in node line");
        }
        let samples = self.integer(fields[1], usize::MAX)?;
        let split = if is_split {
            Some(Split {
                feature: self.integer(fields[2], feature_count.saturating_sub(1))?,
                threshold: self.float(fields[3])?,
                left: self.integer(fields[4], MAX_NODES)?,
                right: self.integer(fields[5], MAX_NODES)?,
            })
        } else {
            None
        };
        let mut counts = Vec::with_capacity(class_count);
        for token in &fields[header_len..] {
            counts.push(self.float(token)?);
        }
        Ok(Node {
            counts,
            samples,
            split,
        })
    }
}

/// Splits on single spaces but gives up (None) once more than `max` fields appear, so a hostile
/// line of spaces cannot allocate memory proportional to its length.
fn bounded_fields(line: &str, max: usize) -> Option<Vec<&str>> {
    let mut parts = line.split(' ');
    let fields: Vec<&str> = parts.by_ref().take(max).collect();
    if parts.next().is_some() {
        None
    } else {
        Some(fields)
    }
}

fn err(line: usize, reason: String) -> CartError {
    CartError::InvalidModel { line, reason }
}

fn err_at<T>(line: usize, reason: &str) -> Result<T, CartError> {
    Err(err(line, reason.to_owned()))
}

/// Shortens an untrusted token for an error message.
fn clip(token: &str) -> String {
    token.chars().take(24).collect()
}
