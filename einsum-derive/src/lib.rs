//! proc-macro based einsum implementation

use einsum_solver::{namespace::*, subscripts::Subscripts};
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use proc_macro_error::{abort_call_site, proc_macro_error, OptionExt};
use quote::quote;

mod args;
mod einsum_fn;
mod ident;

#[cfg(test)]
mod format;

/// proc-macro based einsum
///
/// ```
/// use ndarray::array;
/// use einsum_derive::einsum;
///
/// let a = array![
///   [1.0, 2.0],
///   [3.0, 4.0]
/// ];
/// let b = array![
///   [1.0, 2.0],
///   [3.0, 4.0]
/// ];
/// let c = einsum!("ij,jk->ik", a, b);
/// assert_eq!(c, array![
///   [6.0, 8.0],
///   [12.0, 16.0]
/// ]);
/// ```
///
/// This proc-macro wil compile the input subscripts `"ij,jk->ik"`
/// to generate Rust code executing corresponding operation.
///
/// If the subscripts and the number of input mismatches,
/// this raises compile error:
///
/// ```compile_fail
/// use ndarray::array;
/// use einsum_derive::einsum;
///
/// let a = array![
///   [1.0, 2.0],
///   [3.0, 4.0]
/// ];
/// let c = einsum!("ij,jk->ik", a /* needs one more arg */);
/// ```
#[proc_macro_error]
#[proc_macro]
pub fn einsum(input: TokenStream) -> TokenStream {
    einsum2(input.into()).into()
}

fn einsum2(input: TokenStream2) -> TokenStream2 {
    let (subscripts, args) = args::parse(input);

    // Validate subscripts
    let mut names = Namespace::init();
    let subscripts = Subscripts::from_raw_indices(&mut names, &subscripts)
        .ok()
        .expect_or_abort("Invalid subscripts");
    if subscripts.inputs.len() != args.len() {
        abort_call_site!(
            "Argument number mismatch: subscripts ({}), args ({})",
            subscripts.inputs.len(),
            args.len()
        );
    }

    let einsum_fn = einsum_fn::def_einsum_fn(&subscripts);
    let fn_name = syn::Ident::new(&format!("{}", subscripts), Span::call_site());
    quote! {
        {
            #einsum_fn
            #fn_name(#(#args),*)
        }
    }
}

#[cfg(test)]
mod test {
    use super::{format::format_block, *};
    use std::str::FromStr;

    #[test]
    fn test_snapshots() {
        let input = TokenStream2::from_str(r#""ij,jk->ik", a, b"#).unwrap();
        let tt = format_block(einsum2(input).to_string());
        insta::assert_snapshot!(tt, @r###"
        {
            fn ij_jk__ik<T, S0, S1>(
                arg0: ndarray::ArrayBase<S0, ndarray::Ix2>,
                arg1: ndarray::ArrayBase<S1, ndarray::Ix2>,
            ) -> ndarray::Array<T, ndarray::Ix2>
            where
                T: ndarray::LinalgScalar,
                S0: ndarray::Data<Elem = T>,
                S1: ndarray::Data<Elem = T>,
            {
                let (n_0_0, n_0_1) = arg0.dim();
                let n_i = n_0_0;
                let n_j = n_0_1;
                let (n_1_0, n_1_1) = arg1.dim();
                assert_eq!(n_j, n_1_0);
                let n_k = n_1_1;
                let mut out = ndarray::Array::zeros((n_i, n_k));
                for i in 0..n_i {
                    for k in 0..n_k {
                        for j in 0..n_j {
                            out[(i, k)] = arg0[(i, j)] * arg1[(j, k)];
                        }
                    }
                }
                out
            }
            ij_jk__ik(a, b)
        }
        "###);
    }
}
