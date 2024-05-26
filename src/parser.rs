use crate::{
    rustdoc_types::{
        Crate as RustDocRoot,
        Import,
        Item as RustDocItem,
        ItemEnum as RustDocItemEnum,
        Struct,
        StructKind,
        Variant,
        VariantKind,
        Visibility,
        FORMAT_VERSION,
    },
    seeker::{DocItem, LinkType, RustDoc, TypeItem},
    DocItemKind,
};
use rustc_hash::FxHashMap;
use std::{
    cell::{OnceCell, RefCell},
    collections::BTreeSet,
    iter,
    str::FromStr,
};
use string_cache::DefaultAtom as Atom;
use thiserror::Error;

/// Error type for [`RustDoc`] parsing.
///
/// [`RustDoc`]: RustDoc
#[derive(Debug, Error)]
pub enum RustDocParseError {
    /// Failed to parse the input string as a rustdoc JSON document.
    #[error("invalid input JSON string")]
    Json(#[from] serde_json::Error),
    /// The rustdoc JSON format has an unsupported version.
    #[error("unsupported rustdoc format version: {0}")]
    UnsupportedFormatVersion(u32),
}

impl FromStr for RustDoc {
    type Err = RustDocParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let doc: RustDocRoot = serde_json::from_str(s)?;
        if doc.format_version != FORMAT_VERSION {
            return Err(RustDocParseError::UnsupportedFormatVersion(
                doc.format_version,
            ));
        }

        #[derive(Debug, Clone, Default)]
        enum ItemTypeParent {
            #[default]
            Root,
            ModuleItem {
                path_parent: Atom,
            },
            AssociateItem {
                type_parent: Atom,
            },
            // For a structfield node,
            // /crossterm/style/enum.Color.html#variant.Rgb   .field.r
            //                  ^^^^^^^^^^^^^^^ ^^^^^^^^^^^    ^^^^^^^
            //                  type_parent     associate_item self
            SubAssociateItem {
                type_parent: Atom,
                associate_item: Atom,
            },
        }
        #[derive(Debug, Clone)]
        struct ItemNode {
            item: RustDocItem,
            name: Atom,
            kind: RefCell<DocItemKind>,
            parent: OnceCell<ItemTypeParent>,
            imported_by: RefCell<Vec<Atom>>,
        }

        impl From<&'_ ItemNode> for TypeItem {
            fn from(node: &'_ ItemNode) -> Self {
                TypeItem {
                    kind: *node.kind.borrow(),
                    name: node.name.clone(),
                }
            }
        }

        let nodes = doc
            .index
            .into_iter()
            .map(|(id, item)| {
                (Atom::from(id.0), ItemNode {
                    name: item
                        .name
                        .as_deref()
                        .or(match &item.inner {
                            RustDocItemEnum::Import(import) => Some(import.name.as_str()),
                            _ => None,
                        })
                        .unwrap_or_default()
                        .into(),
                    kind: RefCell::new(map_doc_item_kind(&item)),
                    parent: OnceCell::new(),
                    imported_by: RefCell::new(Vec::new()),
                    item,
                })
            })
            .collect::<FxHashMap<_, _>>();

        if let Some(root) = nodes.get(&Atom::from(&*doc.root.0)) {
            root.parent.set(ItemTypeParent::Root).ok();
        }

        'node_loop: for (id, node) in &nodes {
            use crate::rustdoc_types::{ItemEnum as R, *};

            // Maintain imported_by for Import nodes
            if let RustDocItemEnum::Import(Import {
                id: Some(importee_id),
                ..
            }) = &node.item.inner
            {
                let mut importee_id = Atom::from(&*importee_id.0);
                let importee = loop {
                    let Some(importee) = nodes.get(&importee_id) else {
                        // Importee may not be available in this crate.
                        continue 'node_loop;
                    };
                    if let RustDocItemEnum::Import(Import {
                        id: Some(id), ..
                    }) = &importee.item.inner
                    {
                        importee_id = Atom::from(&*id.0);
                    } else {
                        break importee;
                    }
                };
                importee.imported_by.borrow_mut().push(id.clone());
            }

            // Adjust parents for direct descendants
            match &node.item.inner {
                R::Module(Module {
                    items, ..
                }) => {
                    // prelude modules usually contain non-inline items which do not have an actual
                    // page
                    if &*node.name != "prelude" {
                        items
                            .iter()
                            .filter_map(|item| nodes.get(&Atom::from(&*item.0)))
                            .for_each(|item| {
                                item.parent
                                    .set(ItemTypeParent::ModuleItem {
                                        path_parent: id.clone(),
                                    })
                                    .ok();
                            });
                    }
                },
                R::Union(Union {
                    fields: items, ..
                })
                | R::Struct(Struct {
                    kind:
                        StructKind::Plain {
                            fields: items, ..
                        },
                    ..
                })
                | R::Enum(Enum {
                    variants: items, ..
                })
                | R::Trait(Trait {
                    items, ..
                }) => {
                    items
                        .iter()
                        .filter_map(|item| nodes.get(&Atom::from(&*item.0)))
                        .for_each(|item| {
                            item.parent
                                .set(ItemTypeParent::AssociateItem {
                                    type_parent: id.clone(),
                                })
                                .ok();
                            fix_associated_item_kind(&mut item.kind.borrow_mut(), &item.item);
                        });
                },
                R::Struct(Struct {
                    kind: StructKind::Tuple(items),
                    ..
                })
                | R::Variant(Variant {
                    kind: VariantKind::Tuple(items),
                    ..
                }) => {
                    items
                        .iter()
                        .filter_map(|item| nodes.get(&Atom::from(&*item.as_ref()?.0)))
                        .for_each(|item| {
                            item.parent
                                .set(ItemTypeParent::AssociateItem {
                                    type_parent: id.clone(),
                                })
                                .ok();
                        });
                },
                _ => {},
            }

            // Adjust parents for fields of struct-style enum variants
            if let R::Enum(enum_) = &node.item.inner {
                enum_
                    .variants
                    .iter()
                    .filter_map(|id| {
                        if let R::Variant(Variant {
                            kind:
                                VariantKind::Struct {
                                    fields, ..
                                },
                            ..
                        }) = &nodes.get(&Atom::from(&*id.0))?.item.inner
                        {
                            Some((Atom::from(&*id.0), fields))
                        } else {
                            None
                        }
                    })
                    .flat_map(|(variant_id, fields)| {
                        fields.iter().map(move |field| (variant_id.clone(), field))
                    })
                    .filter_map(|(variant_id, field)| {
                        Some((variant_id, nodes.get(&Atom::from(&*field.0))?))
                    })
                    .for_each(|(variant_id, field)| {
                        field
                            .parent
                            .set(ItemTypeParent::SubAssociateItem {
                                type_parent: id.clone(),
                                associate_item: variant_id,
                            })
                            .ok();
                    });
            }

            // Adjust parents for impl and its items
            if let R::Union(Union {
                impls, ..
            })
            | R::Struct(Struct {
                impls, ..
            })
            | R::Enum(Enum {
                impls, ..
            })
            | R::Primitive(Primitive {
                impls, ..
            }) = &node.item.inner
            {
                impls
                    .iter()
                    .filter_map(|item| nodes.get(&Atom::from(&*item.0)))
                    .inspect(|item| {
                        item.parent
                            .set(ItemTypeParent::AssociateItem {
                                type_parent: id.clone(),
                            })
                            .ok();
                        fix_associated_item_kind(&mut item.kind.borrow_mut(), &item.item);
                    })
                    .filter_map(|item| {
                        if let R::Impl(imp) = &item.item.inner {
                            Some(imp)
                        } else {
                            None
                        }
                    })
                    .flat_map(|imp| &imp.items)
                    .filter_map(|item| nodes.get(&Atom::from(&*item.0)))
                    .for_each(|item| {
                        item.parent
                            .set(ItemTypeParent::AssociateItem {
                                type_parent: id.clone(),
                            })
                            .ok();
                        fix_associated_item_kind(&mut item.kind.borrow_mut(), &item.item);
                    });
            }
        }

        // Cache paths for Module and glob Import nodes
        let mut path_cache = FxHashMap::<Atom, Vec<Atom>>::default();
        let mut items = BTreeSet::new();
        nodes
            .values()
            .filter(|node| !matches!(node.item.visibility, Visibility::Restricted { .. }))
            .filter(|node| {
                // For Import nodes, let the importees to generate duplicates for each Import.
                !matches!(node.item.inner, RustDocItemEnum::Import(_))
            })
            .filter(|node| !matches!(node.item.inner, RustDocItemEnum::Impl(_)))
            .filter_map(|node| {
                let parent = node.parent.get()?;
                if let ItemTypeParent::AssociateItem {
                    type_parent,
                } = parent
                {
                    let type_parent = nodes.get(type_parent)?;
                    if let RustDocItemEnum::Struct(Struct {
                        kind: StructKind::Tuple(_),
                        ..
                    })
                    | RustDocItemEnum::Variant(Variant {
                        kind: VariantKind::Tuple(_),
                        ..
                    }) = &type_parent.item.inner
                    {
                        return None;
                    }
                }
                Some((node, parent))
            })
            .for_each(|(node, parent)| {
                fn generate_path(
                    starting_node: &ItemNode,
                    omit_self: bool,
                    nodes: &FxHashMap<Atom, ItemNode>,
                    path_cache: &mut FxHashMap<Atom, Vec<Atom>>,
                ) -> Vec<Atom> {
                    let cache_key = Atom::from(&*starting_node.item.id.0);
                    if let Some(paths) = path_cache.get(&cache_key).filter(|_| !omit_self) {
                        return paths.clone();
                    }
                    if matches!(starting_node.item.visibility, Visibility::Restricted { .. }) {
                        path_cache.insert(starting_node.name.clone(), vec![]);
                        return vec![];
                    }
                    let mut paths = vec![];
                    let tail = if omit_self
                        || matches!(
                            starting_node.item.inner,
                            RustDocItemEnum::Import(Import {
                                glob: true,
                                ..
                            })
                        ) {
                        "".into()
                    } else {
                        let mut tail = String::with_capacity(starting_node.name.len() + 2);
                        tail.push_str("::");
                        tail.push_str(&starting_node.name);
                        tail
                    };
                    match starting_node.parent.get() {
                        Some(ItemTypeParent::ModuleItem {
                            path_parent,
                        }) => {
                            let parent_paths = nodes
                                .get(path_parent)
                                .into_iter()
                                .flat_map(|parent| generate_path(parent, false, nodes, path_cache));
                            paths.extend(
                                parent_paths.map(|p| p.to_string() + &tail).map(Into::into),
                            );
                        },
                        Some(ItemTypeParent::Root) => {
                            paths.push(Atom::from(tail.trim_start_matches("::")));
                        },
                        _ => (),
                    }

                    paths.reserve(starting_node.imported_by.borrow().len());
                    for import_node in starting_node.imported_by.borrow().iter() {
                        let Some(import_node) = nodes.get(import_node) else {
                            continue;
                        };
                        paths.extend(generate_path(import_node, omit_self, nodes, path_cache));
                    }
                    if !omit_self {
                        path_cache.insert(cache_key, paths.clone());
                    }
                    paths.clone()
                }

                fn append_associate_items(
                    nodes: &FxHashMap<Atom, ItemNode>,
                    node: &ItemNode,
                    type_parent: &Atom,
                    gen_link_type: &mut impl FnMut(TypeItem) -> LinkType,
                    items: &mut BTreeSet<DocItem>,
                    path_cache: &mut FxHashMap<Atom, Vec<Atom>>,
                ) {
                    let Some(type_parent) = nodes.get(type_parent) else {
                        return;
                    };
                    let name = TypeItem::from(node);
                    let desc = Atom::from(node.item.docs.as_deref().unwrap_or_default());
                    let type_parent_typeitem = TypeItem::from(type_parent);
                    let parent_reexports = type_parent.imported_by.borrow();
                    let new_items = parent_reexports
                        .iter()
                        .filter_map(|imported_by| nodes.get(imported_by))
                        .chain(iter::once(type_parent))
                        .flat_map(|parent| {
                            generate_path(parent, true, nodes, path_cache)
                                .into_iter()
                                .map(move |path| (parent, path))
                        })
                        .map(|(type_parent, path)| DocItem {
                            name: name.clone(),
                            link_type: gen_link_type(TypeItem {
                                kind: type_parent_typeitem.kind,
                                name: type_parent.name.clone(),
                            }),
                            desc: desc.clone(),
                            path,
                        });
                    items.extend(new_items);
                }

                match parent {
                    ItemTypeParent::AssociateItem {
                        type_parent,
                    } => {
                        append_associate_items(
                            &nodes,
                            node,
                            type_parent,
                            &mut |typeitem| LinkType::AssociateItem {
                                page_item: typeitem,
                            },
                            &mut items,
                            &mut path_cache,
                        );
                    },
                    ItemTypeParent::SubAssociateItem {
                        type_parent,
                        associate_item,
                    } => {
                        let Some(parent_associate_item) =
                            nodes.get(associate_item).map(TypeItem::from)
                        else {
                            return;
                        };
                        append_associate_items(
                            &nodes,
                            node,
                            type_parent,
                            &mut |typeitem| LinkType::SubAssociateItem {
                                page_item: typeitem,
                                parent: parent_associate_item.clone(),
                            },
                            &mut items,
                            &mut path_cache,
                        );
                    },
                    _ => {
                        let name = TypeItem::from(node);
                        let desc = Atom::from(node.item.docs.as_deref().unwrap_or_default());
                        let new_items = generate_path(node, true, &nodes, &mut path_cache)
                            .into_iter()
                            .map(|path| DocItem {
                                name: name.clone(),
                                link_type: if name.kind == DocItemKind::Module {
                                    LinkType::Index
                                } else {
                                    LinkType::Page
                                },
                                desc: desc.clone(),
                                path,
                            });
                        items.extend(new_items);
                    },
                };
            });

        Ok(RustDoc::new(items))
    }
}

fn map_doc_item_kind(item: &RustDocItem) -> DocItemKind {
    use crate::{
        rustdoc_types::{ItemEnum as R, MacroKind},
        seeker::DocItemKind as K,
    };
    match &item.inner {
        R::Module(_) => K::Module,
        R::ExternCrate {
            ..
        } => K::ExternCrate,
        R::Import(_) => K::Import,
        R::Union(_) => K::Union,
        R::Struct(_) => K::Struct,
        R::StructField(_) => K::StructField,
        R::Enum(_) => K::Enum,
        R::Variant(_) => K::Variant,
        R::Function(_) => K::Function,
        R::Trait(_) => K::Trait,
        R::TraitAlias(_) => K::TraitAlias,
        R::Impl(_) => K::Impl,
        R::TypeAlias(_) => K::Typedef,
        R::OpaqueTy(_) => unimplemented!("don't know how to handle OpaqueTy"),
        R::Constant(_) => K::Constant,
        R::Static(_) => K::Static,
        R::ForeignType => K::ForeignType,
        R::Macro(_) => K::Macro,
        R::ProcMacro(proc_macro) => match proc_macro.kind {
            MacroKind::Bang => K::Macro,
            MacroKind::Attr => K::AttributeMacro,
            MacroKind::Derive => K::DeriveMacro,
        },
        R::Primitive(_) => K::Primitive,
        R::AssocConst {
            ..
        } => K::AssociatedConst,
        R::AssocType {
            ..
        } => K::AssociatedType,
    }
}

fn fix_associated_item_kind(kind: &mut DocItemKind, item: &RustDocItem) {
    use crate::{rustdoc_types::ItemEnum as R, seeker::DocItemKind as K};

    *kind = match (&kind, &item.inner) {
        (K::Function, R::Function(func)) if func.has_body => K::Method,
        (K::Function, R::Function(..)) => K::TyMethod,
        _ => return,
    }
}

impl DocItemKind {
    pub fn is_associated_item(&self) -> bool {
        matches!(
            self,
            DocItemKind::AssociatedConst
                | DocItemKind::AssociatedType
                | DocItemKind::Method
                | DocItemKind::TyMethod
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs;

    #[test]
    fn test_parser() {
        let data = fs::read_to_string("doc-json/proc_macro.json").unwrap();
        let _: RustDoc = data.parse().unwrap();
    }
}
