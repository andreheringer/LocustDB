use super::proc_macro::{TokenStream, Span};
use syn::parse::{Parse, ParseStream, Result};
use syn::token::{Brace, Match};
use syn::punctuated::Punctuated;
use syn::{Arm, Pat, Block, Stmt, parse_macro_input, parse_quote, Expr, Ident, LitStr, Token, ExprMatch};


struct TypeExpand {
    name: LitStr,
    productions: Vec<Production>,
}

#[derive(Clone)]
struct Production {
    specs: Vec<Declaration>,
    expr: Expr,
}

#[derive(Clone)]
struct Declaration {
    variables: Vec<Ident>,
    t: Ident,
}

impl Parse for TypeExpand {
    fn parse(input: ParseStream) -> Result<Self> {
        let name: LitStr = input.parse()?;
        input.parse::<Token![;]>()?;

        let productions = Punctuated::<Production, Token![;]>::parse_separated_nonempty(input)?;

        Ok(TypeExpand {
            name,
            productions: productions.into_iter().collect(),
        })
    }
}

impl Parse for Production {
    fn parse(input: ParseStream) -> Result<Self> {
        let specs = Punctuated::<Declaration, Token![,]>::parse_separated_nonempty(input)?;
        input.parse::<Token![;]>()?;

        let expr: Expr = input.parse()?;

        Ok(Production {
            specs: specs.into_iter().collect(),
            expr,
        })
    }
}

impl Parse for Declaration {
    fn parse(input: ParseStream) -> Result<Self> {
        let variables = Punctuated::<Ident, Token![,]>::parse_separated_nonempty(input)?;
        input.parse::<Token![:]>()?;
        let t: Ident = input.parse()?;

        Ok(Declaration {
            variables: variables.into_iter().collect(),
            t,
        })
    }
}

pub fn reify_types(input: TokenStream) -> TokenStream {
    let TypeExpand {
        name,
        productions,
    } = parse_macro_input!(input as TypeExpand);

    let mut all_match_arms = Vec::new();
    let mut unified_variable_groups = Vec::new();
    let mut type_equalities = Vec::<Stmt>::new();

    for Production { specs, expr } in productions {
        let mut type_domains = Vec::with_capacity(specs.len());
        let mut variable_groups = Vec::with_capacity(specs.len());
        for Declaration { variables, t } in specs {
            if variables.len() > 1 {
                let v0 = variables[0].clone();
                for v in &variables[1..] {
                    let name0 = LitStr::new(&format!("{}", &v0), v0.span());
                    let name1 = LitStr::new(&format!("{}", &v), v.span());
                    type_equalities.push(parse_quote! {
                        if #v0.tag != #v.tag {
                            return Err(
                                fatal!("Expected identical types for `{}` ({:?}) and `{}` ({:?}).",
                                       #name0, #v0.tag,
                                       #name1, #v.tag),
                            )
                        }
                    });
                }
            }
            type_domains.push(match types(&t) {
                Some(ts) => ts,
                None => {
                    t.span().unstable().error(format!("{} is not a valid type.", t)).emit();
                    return TokenStream::new();
                }
            });
            variable_groups.push(variables.clone());
            if unified_variable_groups.len() < variable_groups.len() {
                unified_variable_groups.push(variables);
            } else {
                let i = variable_groups.len() - 1;
                if variable_groups[i] != unified_variable_groups[i] {
                    t.span().unstable().error(format!(
                        "Set of variables must be identical in all type declarations, but found {:?} and {:?}.",
                        variable_groups[i],
                        unified_variable_groups[i])).emit();
                    return TokenStream::new();
                }
            }
        }
        if variable_groups.len() != unified_variable_groups.len() {
            Span::call_site().error(format!(
                "Set of variables must be identical for all type declarations, but {:?} and {:?} have different number of variables.",
                variable_groups,
                unified_variable_groups)).emit();
            return TokenStream::new();
        }

        let mut cross_product = Vec::new();
        let mut indices = vec![0; type_domains.len()];
        'outer: loop {
            cross_product.push(
                indices
                    .iter()
                    .enumerate()
                    .map(|(t, &i)| type_domains[t][i])
                    .collect::<Vec<_>>()
            );

            for i in 0..type_domains.len() {
                indices[i] += 1;
                if indices[i] < type_domains[i].len() {
                    break;
                }
                if i == type_domains.len() - 1 {
                    break 'outer;
                } else {
                    indices[i] = 0;
                }
            }
        }

        let match_arms = cross_product.into_iter().map(|types| {
            let mut pattern = types[0].pattern();
            let mut block: Block = parse_quote!({
                #expr
            });
            for (i, t) in types.into_iter().enumerate() {
                for v in variable_groups[i].clone().into_iter() {
                    block.stmts.insert(block.stmts.len() - 1, t.reify(v));
                }
                if i != 0 {
                    let p2 = t.pattern();
                    pattern = parse_quote!((#pattern, #p2));
                }
            }

            parse_quote!(#pattern => #block)
        }).collect::<Vec<Arm>>();

        all_match_arms.extend(match_arms);
    }

    let variable = unified_variable_groups[0][0].clone();
    let mut match_expr: Expr = if variable == "aggregator" { parse_quote!(#variable) } else { parse_quote!(#variable.tag) };
    for vg in &unified_variable_groups[1..] {
        let variable = vg[0].clone();
        match_expr = if variable == "aggregator" {
            parse_quote!((#match_expr, #variable))
        } else {
            parse_quote!((#match_expr, #variable.tag))
        };
    }

    all_match_arms.push(parse_quote! {
        t => Err(fatal!("{} not supported for type {:?}", #name, t)),
    });

    let expanded = ExprMatch {
        attrs: vec![],
        match_token: Match::default(),
        expr: Box::new(match_expr),
        brace_token: Brace::default(),
        arms: all_match_arms,
    };

    TokenStream::from(quote! {
        #(#type_equalities)*
        #expanded
    })
}

fn types(t: &Ident) -> Option<Vec<Type>> {
    match t.to_string().as_ref() {
        "Str" => Some(vec![Type::Str]),
        "IntegerNoU64" => Some(vec![Type::U8, Type::U16, Type::U32, Type::I64]),
        "NumberNoU64" => Some(vec![Type::U8, Type::U16, Type::U32, Type::I64, Type::F64]),
        "Integer" => Some(vec![Type::U8, Type::U16, Type::U32, Type::U64, Type::I64]),
        "Float" => Some(vec![Type::F64]),
        "NullableInteger" => Some(vec![Type::NullableU8, Type::NullableU16, Type::NullableU32, Type::NullableI64]),
        "NullableFloat" => Some(vec![Type::NullableF64]),
        "Primitive" => Some(vec![Type::U8, Type::U16, Type::U32, Type::U64, Type::I64, Type::F64, Type::Str, Type::OptStr]),
        "NullablePrimitive" => Some(vec![Type::NullableU8, Type::NullableU16, Type::NullableU32, Type::NullableI64, Type::NullableF64, Type::NullableStr]),
        "PrimitiveUSize" => Some(vec![Type::U8, Type::U16, Type::U32, Type::U64, Type::I64, Type::F64, Type::Str, Type::USize]),
        "PrimitiveNoU64" => Some(vec![Type::U8, Type::U16, Type::U32, Type::I64, Type::F64, Type::Str]),
        "Const" => Some(vec![Type::ScalarI64, Type::ScalarStr]),
        "ScalarI64" => Some(vec![Type::ScalarI64]),
        "ScalarStr" => Some(vec![Type::ScalarStr]),
        "IntAggregator" => Some(vec![Type::AggregatorCount, Type::AggregatorSumI64, Type::AggregatorMaxI64, Type::AggregatorMinI64]),
        "FloatAggregator" => Some(vec![Type::AggregatorCount, Type::AggregatorSumF64, Type::AggregatorMaxF64, Type::AggregatorMinF64]),
        _ => None,
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
enum Type {
    U8,
    U16,
    U32,
    U64,
    I64,
    F64,
    Str,
    OptStr,

    NullableU8,
    NullableU16,
    NullableU32,
    NullableI64,
    NullableF64,
    NullableStr,

    ScalarI64,
    ScalarStr,
    USize,

    AggregatorSumI64,
    AggregatorSumF64,
    AggregatorCount,
    AggregatorMaxI64,
    AggregatorMaxF64,
    AggregatorMinI64,
    AggregatorMinF64,
}

impl Type {
    fn pattern(&self) -> Pat {
        match self {
            Type::U8 => parse_quote!(EncodingType::U8),
            Type::U16 => parse_quote!(EncodingType::U16),
            Type::U32 => parse_quote!(EncodingType::U32),
            Type::U64 => parse_quote!(EncodingType::U64),
            Type::I64 => parse_quote!(EncodingType::I64),
            Type::F64 => parse_quote!(EncodingType::F64),
            Type::Str => parse_quote!(EncodingType::Str),
            Type::OptStr => parse_quote!(EncodingType::OptStr),
            Type::NullableU8 => parse_quote!(EncodingType::NullableU8),
            Type::NullableU16 => parse_quote!(EncodingType::NullableU16),
            Type::NullableU32 => parse_quote!(EncodingType::NullableU32),
            Type::NullableI64 => parse_quote!(EncodingType::NullableI64),
            Type::NullableF64 => parse_quote!(EncodingType::NullableF64),
            Type::NullableStr => parse_quote!(EncodingType::NullableStr),
            Type::USize => parse_quote!(EncodingType::USize),
            Type::ScalarI64 => parse_quote!(EncodingType::ScalarI64),
            Type::ScalarStr => parse_quote!(EncodingType::ScalarStr),
            Type::AggregatorCount => parse_quote!(Aggregator::Count),
            Type::AggregatorSumI64 => parse_quote!(Aggregator::SumI64),
            Type::AggregatorSumF64 => parse_quote!(Aggregator::SumF64),
            Type::AggregatorMaxI64 => parse_quote!(Aggregator::MaxI64),
            Type::AggregatorMaxF64 => parse_quote!(Aggregator::MaxF64),
            Type::AggregatorMinI64 => parse_quote!(Aggregator::MinI64),
            Type::AggregatorMinF64 => parse_quote!(Aggregator::MinF64),
        }
    }

    fn reify(&self, variable: Ident) -> Stmt {
        match self {
            Type::U8 => parse_quote!( let #variable = #variable.buffer.u8(); ),
            Type::U16 => parse_quote!( let #variable = #variable.buffer.u16(); ),
            Type::U32 => parse_quote!( let #variable = #variable.buffer.u32(); ),
            Type::U64 => parse_quote!( let #variable = #variable.buffer.u64(); ),
            Type::I64 => parse_quote!( let #variable = #variable.buffer.i64(); ),
            Type::F64 => parse_quote!( let #variable = #variable.buffer.f64(); ),
            Type::Str => parse_quote!( let #variable = #variable.buffer.str(); ),
            Type::OptStr => parse_quote!( let #variable = #variable.buffer.opt_str(); ),
            Type::NullableU8 => parse_quote!( let #variable = #variable.buffer.nullable_u8(); ),
            Type::NullableU16 => parse_quote!( let #variable = #variable.buffer.nullable_u16(); ),
            Type::NullableU32 => parse_quote!( let #variable = #variable.buffer.nullable_u32(); ),
            Type::NullableI64 => parse_quote!( let #variable = #variable.buffer.nullable_i64(); ),
            Type::NullableF64 => parse_quote!( let #variable = #variable.buffer.nullable_f64(); ),
            Type::NullableStr => parse_quote!( let #variable = #variable.buffer.nullable_str(); ),
            Type::USize => parse_quote!( let #variable = #variable.buffer.usize(); ),
            Type::ScalarI64 => parse_quote!( let #variable = #variable.buffer.scalar_i64(); ),
            Type::ScalarStr => parse_quote!( let #variable = #variable.buffer.scalar_str(); ),
            Type::AggregatorCount => parse_quote!( let #variable = PhantomData::<Count>; ),
            Type::AggregatorSumI64 => parse_quote!( let #variable = PhantomData::<SumI64>; ),
            Type::AggregatorSumF64 => parse_quote!( let #variable = PhantomData::<SumF64>; ),
            Type::AggregatorMaxI64 => parse_quote!( let #variable = PhantomData::<MaxI64>; ),
            Type::AggregatorMaxF64 => parse_quote!( let #variable = PhantomData::<MaxF64>; ),
            Type::AggregatorMinI64 => parse_quote!( let #variable = PhantomData::<MinI64>; ),
            Type::AggregatorMinF64 => parse_quote!( let #variable = PhantomData::<MinF64>; ),
        }
    }
}
