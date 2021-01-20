#![allow(unused_variables)]

use crate::{
  term,
  term::Term,
  valus::{
    dll::*,
    eval,
    literal::Literal,
    literal::LitType,
    primop::PrimOp,
    uses::Uses,
  },
};

use core::ptr::NonNull;
use std::{
  alloc::{
    alloc,
    dealloc,
    Layout,
  },
  collections::HashMap,
  convert::TryInto,
  fmt,
};

// A top-down λ-DAG pointer. Keeps track of what kind of node it points to.
#[derive(Clone, Copy)]
pub enum DAG {
  Leaf(NonNull<Leaf>),
  Single(NonNull<Single>),
  Branch(NonNull<Branch>),
}

// Doubly-linked list of parent nodes
type Parents = DLL<ParentCell>;

// A bottom-up (parent) λ-DAG pointer. Keeps track of the relation between
// the child and the parent.
#[derive(Clone, Copy)]
pub enum ParentCell {
  Root,
  Single(NonNull<Single>),
  Left(NonNull<Branch>),
  Right(NonNull<Branch>),
}

// The λ-DAG nodes
pub struct Leaf {
  pub tag: LeafTag,
  pub parents: Option<NonNull<Parents>>,
}

#[derive(Clone)]
pub enum LeafTag {
  Typ,
  LTy(LitType),
  Lit(Literal),
  Opr(PrimOp),
  Var(String),
}
  
pub struct Single {
  pub var: Option<NonNull<Leaf>>,
  pub tag: SingleTag,
  pub single: DAG,
  pub single_ref: NonNull<Parents>,
  pub parents: Option<NonNull<Parents>>,
}

#[derive(Clone)]
pub enum SingleTag {
  Lam,
  Fix,
  Slf,
  Cse,
  Dat,
}

pub struct Branch {
  pub var: Option<NonNull<Leaf>>,
  pub tag: BranchTag,
  pub left: DAG,
  pub right: DAG,
  pub left_ref: NonNull<Parents>,
  pub right_ref: NonNull<Parents>,
  pub copy: Option<NonNull<Branch>>,
  pub parents: Option<NonNull<Parents>>,
}

#[derive(Clone)]
pub enum BranchTag {
  App,
  Ann,
  All(Uses),
}

// Get the parents of a term.
#[inline]
pub fn get_parents(term: DAG) -> Option<NonNull<Parents>> {
  unsafe {
    match term {
      DAG::Leaf(link) => (*link.as_ptr()).parents,
      DAG::Single(link) => (*link.as_ptr()).parents,
      DAG::Branch(link) => (*link.as_ptr()).parents,
    }
  }
}

// Set the parent slot of a term
#[inline]
pub fn set_parents(term: DAG, pref: Option<NonNull<Parents>>) {
  unsafe {
    match term {
      DAG::Leaf(link) => (*link.as_ptr()).parents = pref,
      DAG::Single(link) => (*link.as_ptr()).parents = pref,
      DAG::Branch(link) => (*link.as_ptr()).parents = pref,
    }
  }
}

// Given a term and a parent node, add the node to term's parents.
#[inline]
pub fn add_to_parents(node: DAG, plink: NonNull<Parents>) {
  let parents = get_parents(node);
  match parents {
    Some(parents) => unsafe { (*parents.as_ptr()).merge(plink) },
    None => set_parents(node, Some(plink)),
  }
}

// Resets the cache slots of the app nodes.
pub fn clear_copies(mut spine: &Single, top_branch: &mut Branch) {
  #[inline]
  fn clean_up_var(var: Option<NonNull<Leaf>>) {
    match var {
      Some(var) => {
        let var = &mut *var.as_ptr();
        for var_parent in DLL::iter_option((*var).parents) {
          clean_up(var_parent);
        }
      },
      None => (),
    };
  }
  fn clean_up(cc: &ParentCell) {
    match cc {
      ParentCell::Left(parent) => unsafe {
        let parent = &mut *parent.as_ptr();
        parent.copy.map_or((), |branch| {
          parent.copy = None;
          let Branch { var, left, left_ref, right, right_ref, parents, .. } = *branch.as_ptr();
          add_to_parents(left, left_ref);
          add_to_parents(right, right_ref);
          clean_up_var(var);
          for grandparent in DLL::iter_option(parents) {
            clean_up(grandparent);
          }
        })
      },
      ParentCell::Right(parent) => unsafe {
        let parent = &mut *parent.as_ptr();
        parent.copy.map_or((), |branch| {
          parent.copy = None;
          let Branch { var, left, left_ref, right, right_ref, parents, .. } = *branch.as_ptr();
          add_to_parents(left, left_ref);
          add_to_parents(right, right_ref);
          clean_up_var(var);
          for grandparent in DLL::iter_option(parents) {
            clean_up(grandparent);
          }
        })
      },
      ParentCell::Single(parent) => unsafe {
        let Single { parents, var, .. } = &*parent.as_ptr();
        clean_up_var(*var);
        for grandparent in DLL::iter_option(*parents) {
          clean_up(grandparent);
        }
      },
      ParentCell::Root => (),
    }
  }
  // Clears the top app cache and adds itself to its children's list of parents
  top_branch.copy.map_or((), |ptr| unsafe {
    top_branch.copy = None;
    let Branch { var, left, left_ref, right, right_ref, .. } = *ptr.as_ptr();
    add_to_parents(left, left_ref);
    add_to_parents(right, right_ref);
    clean_up_var(var);
  });
  loop {
    clean_up_var(spine.var);
    match spine.single {
      DAG::Single(single) => unsafe { spine = &*single.as_ptr() },
      _ => break,
    }
  }
}

// // Free parentless nodes.
pub fn free_dead_node(node: DAG) {
  #[inline]
  fn free_var(var: Option<NonNull<Leaf>>) {
    match var {
      Some(var) => {
        if (*var.as_ptr()).parents.is_none() {
          free_dead_node(DAG::Leaf(var))
        }
      },
      None => (),
    };
  }
  unsafe {
    match node {
      DAG::Single(link) => {
        let Single { single, single_ref, var, .. } = &*link.as_ptr();
        free_var(*var);
        let new_single_parents = DLL::remove_node(*single_ref);
        set_parents(*single, new_single_parents);
        match new_single_parents {
          None => free_dead_node(*single),
          _ => (),
        }
        dealloc(link.as_ptr() as *mut u8, Layout::new::<Single>());
      }
      DAG::Branch(link) => {
        let Branch { left, right, left_ref, right_ref, var, .. } = &*link.as_ptr();
        free_var(*var);
        let new_left_parents = DLL::remove_node(*left_ref);
        set_parents(*left, new_left_parents);
        match new_left_parents {
          None => free_dead_node(*left),
          _ => (),
        }
        let new_right_parents = DLL::remove_node(*right_ref);
        set_parents(*right, new_right_parents);
        match new_right_parents {
          None => free_dead_node(*right),
          _ => (),
        }
        dealloc(link.as_ptr() as *mut u8, Layout::new::<Branch>());
      }
      DAG::Leaf(link) => {
        dealloc(link.as_ptr() as *mut u8, Layout::new::<Leaf>());
      }
    }
  }
}

// Replace one child w/another in the tree.
pub fn replace_child(oldchild: DAG, newchild: DAG) {
  #[inline]
  fn install_child(parent: &mut ParentCell, newchild: DAG) {
    unsafe {
      match parent {
        ParentCell::Left(parent) => (*parent.as_ptr()).left = newchild,
        ParentCell::Right(parent) => (*parent.as_ptr()).right = newchild,
        ParentCell::Single(parent) => (*parent.as_ptr()).single = newchild,
        ParentCell::Root => (),
      }
    }
  }
  unsafe {
    let oldpref = get_parents(oldchild);
    if let Some(old_parents) = oldpref {
      let mut iter = (*old_parents.as_ptr()).iter();
      let newpref = get_parents(newchild);
      let mut last_old = None;
      let first_new = newpref.map(|dll| DLL::first(dll));
      while let Some(parent) = iter.next() {
        if iter.is_last() {
          last_old = iter.this();
          last_old.map_or((), |last_old| (*last_old.as_ptr()).next = first_new);
        }
        install_child(parent, newchild);
      }
      first_new.map_or((), |first_new| (*first_new.as_ptr()).prev = last_old);
      set_parents(newchild, oldpref);
    }
    set_parents(oldchild, None);
  }
}

// Allocate memory with a given value in it.
#[inline]
pub fn alloc_val<T>(val: T) -> NonNull<T> {
  unsafe { NonNull::new_unchecked(Box::leak(Box::new(val))) }
}

// Allocate unitialized memory.
#[inline]
pub fn alloc_uninit<T>() -> NonNull<T> {
  unsafe {
    let ptr = alloc(Layout::new::<T>()) as *mut T;
    NonNull::new_unchecked(ptr)
  }
}

// Allocate a fresh branch node, with the two given params as its children.
// Parent references are not added to its children.
#[inline]
pub fn new_branch(oldvar: Option<NonNull<Leaf>>, left: DAG, right: DAG, tag: BranchTag) -> NonNull<Branch> {
  unsafe {
    let left_ref = alloc_uninit();
    let right_ref = alloc_uninit();
    let new_branch = alloc_val(Branch {
      copy: None,
      tag,
      var: None,
      left,
      right,
      left_ref,
      right_ref,
      parents: None,
    });
    *left_ref.as_ptr() = DLL::singleton(ParentCell::Left(new_branch));
    *right_ref.as_ptr() = DLL::singleton(ParentCell::Right(new_branch));
    match oldvar {
      Some(oldvar) => {
        let Leaf { tag: var_tag, parents: var_parents } = &*oldvar.as_ptr();
        let var = alloc_val(Leaf { tag: var_tag.clone(), parents: None });
        (*new_branch.as_ptr()).var = Some(var);
        for parent in DLL::iter_option(*var_parents) {
          eval::upcopy(DAG::Leaf(var), *parent)
        }
      },
      None => (),
    };
    new_branch
  }
}

// Allocate a fresh single node
#[inline]
pub fn new_single(oldvar: Option<NonNull<Leaf>>, single: DAG, tag: SingleTag) -> NonNull<Single> {
  unsafe {
    let single_ref = alloc_uninit();
    let new_single = alloc_val(Single {
      tag,
      var: None,
      single,
      single_ref,
      parents: None,
    });
    *single_ref.as_ptr() = DLL::singleton(ParentCell::Single(new_single));
    add_to_parents(single, single_ref);
    match oldvar {
      Some(oldvar) => {
        let Leaf { tag: var_tag, parents: var_parents } = &*oldvar.as_ptr();
        let var = alloc_val(Leaf { tag: var_tag.clone(), parents: None });
        (*new_single.as_ptr()).var = Some(var);
        for parent in DLL::iter_option(*var_parents) {
          eval::upcopy(DAG::Leaf(var), *parent)
        }
      },
      None => (),
    };
    new_single
  }
}

// Allocate a fresh leaf node
#[inline]
pub fn new_leaf(tag: LeafTag) -> NonNull<Leaf> {
  alloc_val(Leaf { tag, parents: None })
}

// impl fmt::Display for DAG {
//   fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//     write!(f, "{}", self.to_term())
//   }
// }

// impl DAG {
//   pub fn to_term(&self) -> Term {
//     let mut map: HashMap<*mut Var, u64> = HashMap::new();

//     pub fn go(
//       node: &DAG,
//       mut map: &mut HashMap<*mut Var, u64>,
//       depth: u64,
//     ) -> Term {
//       match node {
//         DAG::Var(link) => {
//           let nam = unsafe { &(*link.as_ptr()).name };
//           let level = map.get(&link.as_ptr()).unwrap();
//           Term::Var(None, nam.clone(), depth - level - 1)
//         }
//         DAG::Lam(link) => {
//           let var = unsafe { &(*link.as_ptr()).var };
//           let bod = unsafe { &(*link.as_ptr()).body };
//           let nam = unsafe { &(*var.as_ptr()).name };
//           map.insert(var.as_ptr(), depth);
//           let body = go(bod, &mut map, depth + 1);
//           Term::Lam(None, nam.clone(), Box::new(body))
//         }
//         DAG::App(link) => {
//           let fun = unsafe { &(*link.as_ptr()).func };
//           let arg = unsafe { &(*link.as_ptr()).arg };
//           let fun = go(fun, &mut map, depth);
//           let arg = go(arg, &mut map, depth);
//           Term::App(None, Box::new(fun), Box::new(arg))
//         }
//         DAG::Lit(link) => {
//           let lit = unsafe { &(*link.as_ptr()).val };
//           Term::Lit(None, lit.clone())
//         }
//         DAG::Opr(link) => {
//           let opr = unsafe { &(*link.as_ptr()).opr };
//           Term::Opr(None, *opr)
//         }
//         _ => panic!("TODO"),
//       }
//     }
//     go(&self, &mut map, 0)
//   }

//   pub fn from_term(tree: Term) -> DAG {
//     pub fn go(
//       tree: Term,
//       mut map: HashMap<String, NonNull<Var>>,
//       parents: NonNull<DLL<ParentCell>>,
//     ) -> DAG {
//       match tree {
//         Term::Lam(_, name, body) => {
//           // Allocate nodes
//           let var = alloc_val(Var { name: name.clone(), parents: None });
//           let sons_parents = alloc_uninit();
//           let lam = alloc_val(Lam {
//             var,
//             // Temporary, dangling DAG pointer
//             body: DAG::Lam(NonNull::dangling()),
//             body_ref: sons_parents,
//             parents: Some(parents),
//           });

//           // Update `sons_parents` to refer to current node
//           unsafe {
//             *sons_parents.as_ptr() = DLL::singleton(ParentCell::LamBod(lam));
//           }

//           // Map `name` to `var` node
//           map.insert(name.clone(), var);
//           let body = go(*body, map, sons_parents);

//           // Update `lam` with the correct body
//           unsafe {
//             (*lam.as_ptr()).body = body;
//           }
//           DAG::Lam(lam)
//         }

//         Term::App(_, fun, arg) => {
//           // Allocation and updates
//           let arg_parents = alloc_uninit();
//           let func_parents = alloc_uninit();
//           let app = alloc_val(App {
//             // Temporary, dangling DAG pointers
//             func: DAG::Lam(NonNull::dangling()),
//             arg: DAG::Lam(NonNull::dangling()),
//             func_ref: func_parents,
//             arg_ref: arg_parents,
//             copy: None,
//             parents: Some(parents),
//           });
//           unsafe {
//             *arg_parents.as_ptr() = DLL::singleton(ParentCell::AppArg(app));
//             *func_parents.as_ptr() = DLL::singleton(ParentCell::AppFun(app));
//           }

//           let fun = go(*fun, map.clone(), func_parents);
//           let arg = go(*arg, map, arg_parents);

//           // Update `app` with the correct fields
//           unsafe {
//             (*app.as_ptr()).arg = arg;
//             (*app.as_ptr()).func = fun;
//           }
//           DAG::App(app)
//         }

//         Term::Var(_, name, _) => {
//           let var = match map.get(&name.clone()) {
//             Some(var) => unsafe {
//               DLL::concat(parents, (*var.as_ptr()).parents);
//               (*var.as_ptr()).parents = Some(parents);
//               *var
//             },
//             None => {
//               alloc_val(Var { name: name.clone(), parents: Some(parents) })
//             }
//           };
//           DAG::Var(var)
//         }
//         Term::Lit(_, lit) => {
//           DAG::Lit(alloc_val(Lit { val: lit, parents: Some(parents) }))
//         }
//         Term::Opr(_, opr) => {
//           DAG::Opr(alloc_val(Opr { opr, parents: Some(parents) }))
//         }
//         _ => panic!("TODO: implement Term::to_dag variants"),
//       }
//     }
//     let root = alloc_val(DLL::singleton(ParentCell::Root));
//     go(tree, HashMap::new(), root)
//   }
// }

// #[cfg(test)]
// mod test {
//   use super::*;

//   #[quickcheck]
//   fn term_encode_decode(x: Term) -> bool {
//     println!("x: {}", x);
//     println!("x: {:?}", x);
//     let y = DAG::to_term(&DAG::from_term(x.clone()));
//     println!("y: {}", y);
//     println!("y: {:?}", y);
//     x == y
//   }
// }
