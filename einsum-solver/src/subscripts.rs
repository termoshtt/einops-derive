//! Einsum subscripts, e.g. `ij,jk->ik`
use crate::parser;
use anyhow::{bail, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Subscript {
    raw: parser::RawSubscript,
    position: Position,
}

impl Subscript {
    pub fn raw(&self) -> &parser::RawSubscript {
        &self.raw
    }

    pub fn position(&self) -> &Position {
        &self.position
    }

    pub fn indices(&self) -> Vec<char> {
        match &self.raw {
            parser::RawSubscript::Indices(indices) => indices.clone(),
            parser::RawSubscript::Ellipsis { start, end } => {
                start.iter().chain(end.iter()).cloned().collect()
            }
        }
    }
}

/// Names of tensors
///
/// As the crate level document explains,
/// einsum factorization requires to track names of tensors
/// in addition to subscripts, and this struct manages it.
/// This works as a simple counter, which counts how many intermediate
/// tensor denoted `out{N}` appears and issues new `out{N+1}` identifier.
///
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Namespace {
    last: usize,
}

impl Namespace {
    /// Create new namespace
    pub fn init() -> Self {
        Namespace { last: 0 }
    }

    /// Issue new identifier
    pub fn new(&mut self) -> Position {
        let pos = Position::Intermidiate(self.last);
        self.last += 1;
        pos
    }
}

/// Which tensor the subscript specifies
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum Position {
    /// The tensor which user inputs as N-th argument of einsum
    User(usize),
    /// The tensor created by einsum in its N-th step
    Intermidiate(usize),
}

/// Einsum subscripts with tensor names, e.g. `ij,jk->ik | arg0 arg1 -> out`
#[derive(Debug, PartialEq, Eq)]
pub struct Subscripts {
    /// Input subscript, `ij` and `jk`
    pub inputs: Vec<Subscript>,
    /// Output subscript.
    pub output: Subscript,
}

// `Display` implementation is designed to use function name.
// This is not injective, e.g. `i...,j->ij` and `i,...j->ij`
// returns a same result `i____j__ij`.
impl fmt::Display for Subscripts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for input in &self.inputs {
            write!(f, "{}", input.raw)?;
            write!(f, "_")?;
        }
        write!(f, "_{}", self.output.raw)?;
        Ok(())
    }
}

impl Subscripts {
    /// Normalize subscripts into "explicit mode"
    ///
    /// [numpy.einsum](https://numpy.org/doc/stable/reference/generated/numpy.einsum.html)
    /// has "explicit mode" including `->`, e.g. `ij,jk->ik` and
    /// "implicit mode" e.g. `ij,jk`.
    /// The output subscript is determined from input subscripts in implicit mode:
    ///
    /// > In implicit mode, the chosen subscripts are important since the axes
    /// > of the output are reordered alphabetically.
    /// > This means that `np.einsum('ij', a)` doesn’t affect a 2D array,
    /// > while `np.einsum('ji', a)` takes its transpose.
    /// > Additionally, `np.einsum('ij,jk', a, b)` returns a matrix multiplication,
    /// > while, `np.einsum('ij,jh', a, b)` returns the transpose of
    /// > the multiplication since subscript ‘h’ precedes subscript ‘i’.
    ///
    /// ```
    /// use std::str::FromStr;
    /// use einsum_solver::{subscripts::{Subscripts, Namespace}, parser::RawSubscripts};
    ///
    /// let mut names = Namespace::init();
    ///
    /// // Infer output subscripts for implicit mode
    /// let raw = RawSubscripts::from_str("ij,jk").unwrap();
    /// let subscripts = Subscripts::from_raw(&mut names, raw);
    /// assert_eq!(subscripts.output.raw(), &['i', 'k']);
    ///
    /// // Reordered alphabetically
    /// let raw = RawSubscripts::from_str("ji").unwrap();
    /// let subscripts = Subscripts::from_raw(&mut names, raw);
    /// assert_eq!(subscripts.output.raw(), &['i', 'j']);
    /// ```
    ///
    pub fn from_raw(names: &mut Namespace, raw: parser::RawSubscripts) -> Self {
        let inputs = raw
            .inputs
            .iter()
            .enumerate()
            .map(|(i, indices)| Subscript {
                raw: indices.clone(),
                position: Position::User(i),
            })
            .collect();
        let position = names.new();
        if let Some(output) = raw.output {
            return Subscripts {
                inputs,
                output: Subscript {
                    raw: output,
                    position,
                },
            };
        }

        let count = count_indices(&inputs);
        let output = Subscript {
            raw: parser::RawSubscript::Indices(
                count
                    .iter()
                    .filter_map(|(key, value)| if *value == 1 { Some(*key) } else { None })
                    .collect(),
            ),
            position,
        };
        Subscripts { inputs, output }
    }

    pub fn from_raw_indices(names: &mut Namespace, indices: &str) -> Result<Self> {
        let raw = parser::RawSubscripts::from_str(indices)?;
        Ok(Self::from_raw(names, raw))
    }

    /// Indices to be factorize
    ///
    /// ```
    /// use std::str::FromStr;
    /// use maplit::btreeset;
    /// use einsum_solver::subscripts::{Subscripts, Namespace};
    ///
    /// let mut names = Namespace::init();
    ///
    /// // Matrix multiplication AB
    /// let subscripts = Subscripts::from_raw_indices(&mut names, "ij,jk->ik").unwrap();
    /// assert_eq!(subscripts.contraction_indices(), btreeset!{'j'});
    ///
    /// // Reduce all Tr(AB)
    /// let subscripts = Subscripts::from_raw_indices(&mut names, "ij,ji->").unwrap();
    /// assert_eq!(subscripts.contraction_indices(), btreeset!{'i', 'j'});
    ///
    /// // Take diagonal elements
    /// let subscripts = Subscripts::from_raw_indices(&mut names, "ii->i").unwrap();
    /// assert_eq!(subscripts.contraction_indices(), btreeset!{});
    /// ```
    pub fn contraction_indices(&self) -> BTreeSet<char> {
        let count = count_indices(&self.inputs);
        let mut subscripts: BTreeSet<char> = count
            .into_iter()
            .filter_map(|(key, value)| if value > 1 { Some(key) } else { None })
            .collect();
        for c in &self.output.indices() {
            subscripts.remove(c);
        }
        subscripts
    }

    /// Factorize subscripts
    ///
    /// This requires mutable reference to [Namespace] since factorization process
    /// creates new identifier for intermediate storage, e.g.
    ///
    /// ```text
    /// ij,jk,kl->il | arg0 arg1 arg2 -> out0
    /// ```
    ///
    /// will be factorized into
    ///
    /// ```text
    /// ij,jk->ik | arg0 arg1 -> out1
    /// ik,kl->il | out1 arg2 -> out0
    /// ```
    ///
    /// where `out1` is a new identifier.
    ///
    /// Output
    /// -------
    /// - `Ok(Some(first, second))`: factorization along `index` succeeded
    /// - `Ok(None)`: this subscripts is not `index`-factorizable
    /// - `Err(anyhow::Error)`: `index` is not a contraction index of this subscripts
    ///
    /// When factorization of `origin` along `index` yields two subscripts
    /// `(first, second)` successfully,
    ///
    /// - All subscript in `first` contains `index`
    ///   - `first.contraction_indices()` is a single value set `{ index }`
    ///     because indices except `index` appear as output.
    ///   - `first` is irreducible
    /// - All subscript in `second` does not contains `index`
    ///   - `second.contraction_indices()` is exactly smaller than
    ///     `origin.contraction_indices()`
    ///
    /// Example
    /// --------
    ///
    /// ```
    /// use einsum_solver::{subscripts::*, parser::RawSubscript};
    /// use std::str::FromStr;
    ///
    /// let mut names = Namespace::init();
    /// let base = Subscripts::from_raw_indices(&mut names, "ij,jk,kl->il").unwrap();
    ///
    /// // Factorize along j
    /// let (ijjk, ikkl) = base.factorize(&mut names, 'j').unwrap().unwrap();
    ///
    /// let arg0 = &ijjk.inputs[0];
    /// assert_eq!(arg0.raw(), &RawSubscript::Indices(vec!['i', 'j']));
    /// assert_eq!(arg0.position(), &Position::User(0));
    ///
    /// let arg1 = &ijjk.inputs[1];
    /// assert_eq!(arg1.raw(), &RawSubscript::Indices(vec!['j', 'k']));
    /// assert_eq!(arg1.position(), &Position::User(1));
    ///
    /// let out1 = &ijjk.output;
    /// assert_eq!(out1.raw(), &RawSubscript::Indices(vec!['i', 'k']));
    /// assert_eq!(out1.position(), &Position::Intermidiate(1));
    ///
    /// // returns `Ok(None)` if subscript is irreducible
    /// assert!(ijjk.factorize(&mut names, 'j').unwrap().is_none());
    /// assert!(ikkl.factorize(&mut names, 'k').unwrap().is_none());
    /// ```
    pub fn factorize(&self, names: &mut Namespace, index: char) -> Result<Option<(Self, Self)>> {
        if !self.contraction_indices().contains(&index) {
            bail!("Unknown index: {}", index);
        }

        let mut first = Vec::new();
        let mut second = Vec::new();
        let mut out_indices = BTreeSet::new();
        for input in &self.inputs {
            let indices = input.indices();
            if indices.iter().any(|label| *label == index) {
                first.push(input.clone());
                for c in indices {
                    if c != index {
                        out_indices.insert(c);
                    }
                }
            } else {
                second.push(input.clone());
            }
        }

        // irreducible
        if second.is_empty() {
            return Ok(None);
        }

        let output = Subscript {
            raw: parser::RawSubscript::Indices(out_indices.into_iter().collect()),
            position: names.new(),
        };

        second.insert(0, output.clone());
        Ok(Some((
            Self {
                inputs: first,
                output,
            },
            Self {
                inputs: second,
                output: self.output.clone(),
            },
        )))
    }
}

fn count_indices(inputs: &[Subscript]) -> BTreeMap<char, u32> {
    let mut count = BTreeMap::new();
    for input in inputs {
        for c in input.indices() {
            count.entry(c).and_modify(|n| *n += 1).or_insert(1);
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let mut names = Namespace::init();

        let subscripts = Subscripts::from_raw_indices(&mut names, "ij,jk->ik").unwrap();
        assert_eq!(format!("{}", subscripts), "ij_jk__ik");

        // implicit mode
        let subscripts = Subscripts::from_raw_indices(&mut names, "ij,jk").unwrap();
        assert_eq!(format!("{}", subscripts), "ij_jk__ik");

        // output scalar
        let subscripts = Subscripts::from_raw_indices(&mut names, "i,i").unwrap();
        assert_eq!(format!("{}", subscripts), "i_i__");

        // ellipsis
        let subscripts = Subscripts::from_raw_indices(&mut names, "ij...,jk...->ik...").unwrap();
        assert_eq!(format!("{}", subscripts), "ij____jk_____ik___");
    }
}
