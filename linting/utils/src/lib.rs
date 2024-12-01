// Copyright (C) Use Ink (UK) Ltd.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![doc(
    html_logo_url = "https://use.ink/img/crate-docs/logo.png",
    html_favicon_url = "https://use.ink/crate-docs/favicon.png"
)]
#![feature(rustc_private)]
#![feature(box_patterns)]

extern crate rustc_ast;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_mir_dataflow;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_type_ir;

pub use parity_clippy_utils as clippy;

use clippy::match_def_path;
use if_chain::if_chain;
use rustc_hir::{
    ExprKind,
    HirId,
    ItemId,
    ItemKind,
    QPath,
    StmtKind,
    Ty,
    TyKind,
};
use rustc_lint::LateContext;

/// Returns `true` iff the ink storage attribute is defined for the given HIR
///
/// # Developer Note
///
/// In ink! 5.0.0 our code generation added the annotation
/// `#[cfg(not(feature = "__ink_dylint_Storage"))] to contracts. This
/// allowed dylint to identify the storage struct in a contract.
///
/// Starting with Rust 1.81, `cargo` throws a warning for features that
/// are not declared in the `Cargo.toml` and also for not well-known
/// key-value pairs.
///
/// We don't want to burden contract developers with putting features that
/// are just for internal use there. The only alternative we found is to
/// use an obscure `cfg` condition, that is highly unlikely to be ever
/// annotated in a contract by a developer. Hence, we decided to use
/// `#[cfg(not(target_vendor = "fortanix"))]`, as it seems unlikely that a
/// contract will ever be compiled for this target.
///
/// We have to continue checking for the `__ink_dylint_Storage` attribute
/// here, as the linting will otherwise stop working for ink! 5.0.0 contracts.
fn has_storage_attr(cx: &LateContext, hir: HirId) -> bool {
    const INK_STORAGE_1: &str = "__ink_dylint_Storage";
    const INK_STORAGE_2: &str = "fortanix";
    let attrs = format!("{:?}", cx.tcx.hir().attrs(hir));
    attrs.contains(INK_STORAGE_1) || attrs.contains(INK_STORAGE_2)
}

/// Returns `ItemId` of the structure annotated with `#[ink(storage)]`
pub fn find_storage_struct(cx: &LateContext, item_ids: &[ItemId]) -> Option<ItemId> {
    item_ids
        .iter()
        .find(|&item_id| {
            let item = cx.tcx.hir().item(*item_id);
            if_chain! {
                if has_storage_attr(cx, item.hir_id());
                if let ItemKind::Struct(..) = item.kind;
                then { true } else { false }

            }
        })
        .copied()
}

/// Returns `ItemId`s defined inside the code block of `const _: () = {}`.
///
/// The Rust code expanded after ink! code generation used these to define different
/// implementations of a contract.
fn items_in_unnamed_const(cx: &LateContext<'_>, id: &ItemId) -> Vec<ItemId> {
    if_chain! {
        if let ItemKind::Const(ty, _, body_id) = cx.tcx.hir().item(*id).kind;
        if let TyKind::Tup([]) = ty.kind;
        let body = cx.tcx.hir().body(body_id);
        if let ExprKind::Block(block, _) = body.value.kind;
        then {
            block.stmts.iter().fold(Vec::new(), |mut acc, stmt| {
                if let StmtKind::Item(id) = stmt.kind {
                    // We don't call `items_in_unnamed_const` here recursively, because the source
                    // code generated by ink! doesn't have nested `const _: () = {}` expressions.
                    acc.push(id)
                }
                acc
            })
        } else {
            vec![]
        }
    }
}

/// Collect all the `ItemId`s in nested `const _: () = {}`
pub fn expand_unnamed_consts(cx: &LateContext<'_>, item_ids: &[ItemId]) -> Vec<ItemId> {
    item_ids.iter().fold(Vec::new(), |mut acc, item_id| {
        acc.push(*item_id);
        acc.append(&mut items_in_unnamed_const(cx, item_id));
        acc
    })
}

/// Finds type of the struct that implements a contract with user-defined code
fn find_contract_ty_hir<'tcx>(
    cx: &LateContext<'tcx>,
    item_ids: &[ItemId],
) -> Option<&'tcx Ty<'tcx>> {
    item_ids
        .iter()
        .find_map(|item_id| {
            if_chain! {
                let item = cx.tcx.hir().item(*item_id);
                if let ItemKind::Impl(item_impl) = &item.kind;
                if let Some(trait_ref) = cx.tcx.impl_trait_ref(item.owner_id);
                if match_def_path(
                    cx,
                    trait_ref.skip_binder().def_id,
                    &["ink_env", "contract", "ContractEnv"],
                );
                then { Some(&item_impl.self_ty) } else { None }
            }
        })
        .copied()
}

/// Compares types of two user-defined structs
fn eq_hir_struct_tys(lhs: &Ty<'_>, rhs: &Ty<'_>) -> bool {
    match (lhs.kind, rhs.kind) {
        (
            TyKind::Path(QPath::Resolved(_, lhs_path)),
            TyKind::Path(QPath::Resolved(_, rhs_path)),
        ) => lhs_path.res.eq(&rhs_path.res),
        _ => false,
    }
}

/// Finds an ID of the implementation of the contract struct containing user-defined code
pub fn find_contract_impl_id(
    cx: &LateContext<'_>,
    item_ids: Vec<ItemId>,
) -> Option<ItemId> {
    let contract_struct_ty = find_contract_ty_hir(cx, &item_ids)?;
    item_ids
        .iter()
        .find(|item_id| {
            if_chain! {
                let item = cx.tcx.hir().item(**item_id);
                if let ItemKind::Impl(item_impl) = &item.kind;
                if item_impl.of_trait.is_none();
                if eq_hir_struct_tys(contract_struct_ty, item_impl.self_ty);
                then { true } else { false }
            }
        })
        .copied()
}
