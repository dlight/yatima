use core::ptr::NonNull;

use crate::{
  core::{
    dag::*,
    upcopy::*,
    dll::*,
    primop::{
      apply_bin_op,
      apply_una_op,
    },
    uses::Uses,
  },
  term::{
    // Term,
    Def,
    Link,
  },
};

use im::{
  Vector,
  HashMap,
};

enum Single {
  Lam(Var),
  Slf(Var),
  Dat,
  Cse,
}

enum Branch {
  All(NonNull<All>),
  App(NonNull<App>),
  Ann(NonNull<Ann>),
  Let(NonNull<Let>),
}

// Substitute a variable
pub fn subst(lam: NonNull<Lam>, arg: DAGPtr) -> DAGPtr {
  let Lam { var, bod, parents, .. } = unsafe { &mut *lam.as_ptr() };
  let ans = if DLL::is_singleton(*parents) {
    replace_child(DAGPtr::Var(NonNull::new(var).unwrap()), arg);
    // We have to read `body` again because `lam`'s body could be mutated
    // through `replace_child`
    unsafe {
      (*lam.as_ptr()).bod
    }
  }
  else if var.parents.is_none() {
    *bod
  }
  else {
    let mut input = *bod;
    let mut top_branch = None;
    let mut result = arg;
    let mut spine = vec![];
    loop {
      match input {
        DAGPtr::Lam(link) => {
          let Lam { var, bod, .. } = unsafe { link.as_ref() };
          input = *bod;
          spine.push(Single::Lam(var.clone()));
        }
        DAGPtr::Slf(link) => {
          let Slf { var, bod, .. } = unsafe { link.as_ref() };
          input = *bod;
          spine.push(Single::Slf(var.clone()));
        }
        DAGPtr::Dat(link) => {
          let Dat { bod, .. } = unsafe { link.as_ref() };
          input = *bod;
          spine.push(Single::Dat);
        }
        DAGPtr::Cse(link) => {
          let Cse { bod, .. } = unsafe { link.as_ref() };
          input = *bod;
          spine.push(Single::Cse);
        }
        DAGPtr::App(link) => {
          let App { fun, arg: app_arg, .. } = unsafe { link.as_ref() };
          let new_app = alloc_app(*fun, *app_arg, None);
          unsafe {
            (*link.as_ptr()).copy = Some(new_app);
          }
          top_branch = Some(Branch::App(link));
          for parent in DLL::iter_option(var.parents) {
            upcopy(arg, *parent);
          }
          result = DAGPtr::App(new_app);
          break;
        }
        DAGPtr::All(link) => {
          let All { var: old_var, uses, dom, img, .. } = unsafe { link.as_ref() };
          let Var { nam, dep, parents: var_parents } = old_var;
          let new_var = Var { nam: nam.clone(), dep: *dep, parents: None };
          let new_all = alloc_all(new_var, *uses, *dom, *img, None);
          unsafe {
            (*link.as_ptr()).copy = Some(new_all);
            let ptr: *mut Var = &mut (*new_all.as_ptr()).var;
            for parent in DLL::iter_option(old_var.parents) {
              upcopy(DAGPtr::Var(NonNull::new(ptr).unwrap()), *parent)
            }
          }
          top_branch = Some(Branch::All(link));
          for parent in DLL::iter_option(var.parents) {
            upcopy(arg, *parent);
          }
          result = DAGPtr::All(new_all);
          break;
        }
        DAGPtr::Ann(link) => {
          let Ann { typ, exp, .. } = unsafe { link.as_ref() };
          let new_ann = alloc_ann(*typ, *exp, None);
          unsafe {
            (*link.as_ptr()).copy = Some(new_ann);
          }
          top_branch = Some(Branch::Ann(link));
          for parent in DLL::iter_option(var.parents) {
            upcopy(arg, *parent);
          }
          result = DAGPtr::Ann(new_ann);
          break;
        }
        DAGPtr::Let(link) => panic!("todo"),
        // Otherwise it must be `var`, since `var` necessarily appears inside `body`
        _ => break,
      }
    }
    while let Some(single) = spine.pop() {
      match single {
        Single::Lam(var) => {
          let Var { nam, dep, parents: var_parents } = var;
          let new_var = Var { nam, dep, parents: None };
          let new_lam = alloc_lam(new_var, result, None);
          let ptr: *mut Parents =  unsafe { &mut (*new_lam.as_ptr()).bod_ref };
          add_to_parents(result, NonNull::new(ptr).unwrap());
          let ptr: *mut Var = unsafe { &mut (*new_lam.as_ptr()).var };
          for parent in DLL::iter_option(var_parents) {
            upcopy(DAGPtr::Var(NonNull::new(ptr).unwrap()), *parent)
          }
          result = DAGPtr::Lam(new_lam);
        },
        Single::Slf(var) => {
          let Var { nam, dep, parents: var_parents } = var;
          let new_var = Var { nam, dep, parents: None };
          let new_slf = alloc_slf(new_var, result, None);
          let ptr: *mut Parents =  unsafe { &mut (*new_slf.as_ptr()).bod_ref };
          add_to_parents(result, NonNull::new(ptr).unwrap());
          let ptr: *mut Var = unsafe { &mut (*new_slf.as_ptr()).var };
          for parent in DLL::iter_option(var_parents) {
            upcopy(DAGPtr::Var(NonNull::new(ptr).unwrap()), *parent)
          }
          result = DAGPtr::Slf(new_slf);
        },
        Single::Dat => {
          let new_dat = alloc_dat(result, None);
          let ptr: *mut Parents =  unsafe { &mut (*new_dat.as_ptr()).bod_ref };
          add_to_parents(result, NonNull::new(ptr).unwrap());
          result = DAGPtr::Dat(new_dat);
        },
        Single::Cse => {
          let new_cse = alloc_cse(result, None);
          let ptr: *mut Parents =  unsafe { &mut (*new_cse.as_ptr()).bod_ref };
          add_to_parents(result, NonNull::new(ptr).unwrap());
          result = DAGPtr::Cse(new_cse);
        },
      }
    }
    // If the top branch is non-null, then clear the copies and fix the uplinks
    if let Some(top_branch) = top_branch {
      match top_branch {
        Branch::App(link) => unsafe {
          let top_app = &mut *link.as_ptr();
          top_app.copy.map_or((), |link| {
            top_app.copy = None;
            let App { fun, fun_ref, arg, arg_ref, .. } = &mut *link.as_ptr();
            add_to_parents(*fun, NonNull::new(fun_ref).unwrap());
            add_to_parents(*arg, NonNull::new(arg_ref).unwrap());
          });
        }
        Branch::All(link) => unsafe {
          let top_all = &mut *link.as_ptr();
          top_all.copy.map_or((), |link| {
            top_all.copy = None;
            let All { var, dom, dom_ref, img, img_ref, .. } = &mut *link.as_ptr();
            add_to_parents(*dom, NonNull::new(dom_ref).unwrap());
            add_to_parents(*img, NonNull::new(img_ref).unwrap());
            for var_parent in DLL::iter_option(var.parents) {
              clean_up(var_parent);
            }
          });
        }
        Branch::Ann(link) => unsafe {
          let top_ann = &mut *link.as_ptr();
          top_ann.copy.map_or((), |link| {
            top_ann.copy = None;
            let Ann { typ, typ_ref, exp, exp_ref, .. } = &mut *link.as_ptr();
            add_to_parents(*typ, NonNull::new(typ_ref).unwrap());
            add_to_parents(*exp, NonNull::new(exp_ref).unwrap());
          });
        }
        Branch::Let(link) => panic!("todo"),
      }
      let mut spine = DAGPtr::Lam(lam);
      loop {
        match spine {
          DAGPtr::Lam(link) => unsafe {
            let Lam { var, bod, .. } = &mut *link.as_ptr();
            for var_parent in DLL::iter_option(var.parents) {
              clean_up(var_parent);
            }
            spine = *bod;
          },
          DAGPtr::Slf(link) => unsafe {
            let Slf { var, bod, .. } = &mut *link.as_ptr();
            for var_parent in DLL::iter_option(var.parents) {
              clean_up(var_parent);
            }
            spine = *bod;
          },
          DAGPtr::Dat(link) => unsafe {
            spine = link.as_ref().bod;
          },
          DAGPtr::Cse(link) => unsafe {
            spine = link.as_ref().bod;
          },
          _ => break,
        }
      }
    }
    result
  };
  ans
}

// Contract a lambda redex, return the body.
pub fn reduce_lam(redex: NonNull<App>, lam: NonNull<Lam>) -> DAGPtr {
  let App { arg, .. } = unsafe { redex.as_ref() };
  let top_node = subst(lam, *arg);
  replace_child(DAGPtr::App(redex), top_node);
  free_dead_node(DAGPtr::App(redex));
  top_node
}

impl DAG {
  // Reduce term to its weak head normal form
  pub fn whnf(&mut self, defs: &HashMap<Link, Def>) {
    let mut node = self.head;
    let mut trail = vec![];
    loop {
      match node {
        DAGPtr::App(link) => {
          let App { fun, .. } = unsafe { link.as_ref() };
          trail.push(link);
          node = *fun;
        },
        DAGPtr::Lam(link) => {
          if let Some(app_link) = trail.pop() {
            node = reduce_lam(app_link, link);
          }
          else {
            break;
          }
        },
        DAGPtr::Cse(link) => {
          let mut body = unsafe { DAG::new((*link.as_ptr()).bod) };
          body.whnf(defs);
          match body.head {
            DAGPtr::Dat(body_link) => {
              let Dat { bod: single_body, .. } = unsafe { body_link.as_ref() };
              replace_child(node, *single_body);
              free_dead_node(node);
              node = *single_body;
            },
            _ => break,
          }
        },
      //       SingleTag::Fix => {
      //         let body = (*link.as_ptr()).body;
      //         match var {
      //           None => panic!("Malformed Fix"),
      //           Some(var) => {
      //             let Var { parents: var_parents, depth: var_depth, .. } = *var.as_ptr();
      //             let var_name = &(*var.as_ptr()).name;
      //             replace_child(node, body);
      //             if !var_parents.is_none() {
      //               let new_var = alloc_val(Var {name: var_name.clone(), depth: var_depth, parents: None});
      //               let mut input = body;
      //               let mut top_branch = None;
      //               let mut result = DAGPtr::Var(new_var);
      //               let mut spine = vec![];
      //               loop {
      //                 match input {
      //                   DAGPtr::Single(single) => {
      //                     let Single { var, body, .. } = *single.as_ptr();
      //                     let tag = &(*single.as_ptr()).tag;
      //                     input = body;
      //                     spine.push((var, tag));
      //                   }
      //                   DAGPtr::Branch(branch) => {
      //                     let Branch { left, right, .. } = *branch.as_ptr();
      //                     let new_branch = upcopy_branch(branch, left, right);
      //                     top_branch = Some(branch);
      //                     for parent in DLL::iter_option(var_parents) {
      //                       upcopy(DAGPtr::Var(new_var), *parent);
      //                     }
      //                     result = DAGPtr::Branch(new_branch);
      //                     break;
      //                   }
      //                   // Otherwise it must be `var`, since `var` necessarily appears inside
      //                   // `body`
      //                   _ => break,
      //                 }
      //               }
      //               if top_branch.is_none() && spine.is_empty() {
      //                 panic!("Infinite loop found");
      //               }
      //               while let Some((var, tag)) = spine.pop() {
      //                 result = DAGPtr::Single(upcopy_single(var, result, tag.clone()));
      //               }
      //               top_branch
      //                 .map_or((), |mut app| clear_copies(link.as_ref(), app.as_mut()));

      //               // Create a new fix node with the result of the copy
      //               let fix_ref = alloc_uninit();
      //               let new_fix = alloc_val(Single {
      //                 tag: SingleTag::Fix,
      //                 var: Some(new_var),
      //                 body: result,
      //                 body_ref: fix_ref,
      //                 parents: None
      //               });
      //               *fix_ref.as_ptr() = DLL::singleton(ParentPtr::Body(new_fix));
      //               add_to_parents(result, fix_ref);
      //               replace_child(DAGPtr::Var(var), DAGPtr::Single(new_fix));
      //             }
      //             free_dead_node(node);
      //             node = body;
      //           },
      //         };
      //       }
      //       _ => break,
      //     }
      //   },

        // LeafTag::Ref(nam, def_link, _anon_link) => {
        //   if let Some(def) = defs.get(def_link) {
        //     // Using Fix:
        //     let new_var = alloc_val(Var {name: nam.clone(), depth: 0, parents: None});
        //     let new_node = DAG::from_subterm(&def.clone().term, 0, &Vector::new(), Vector::unit(DAGPtr::Var(new_var)), None).head;
        //     let fix_ref = alloc_uninit();
        //     let new_fix = alloc_val(Single {
        //       tag: SingleTag::Fix,
        //       var: Some(new_var),
        //       body: new_node,
        //       body_ref: fix_ref,
        //       parents: None
        //     });
        //     *fix_ref.as_ptr() = DLL::singleton(ParentPtr::Body(new_fix));
        //     add_to_parents(new_node, fix_ref);
        //     replace_child(node, DAGPtr::Single(new_fix));
        //     free_dead_node(node);
        //     node = DAGPtr::Single(new_fix);
        //   }
        //   else {
        //     panic!("undefined runtime reference: {}, {}", nam, def_link);
        //   }
        // }

        DAGPtr::Opr(link) => {
          let opr = unsafe { (*link.as_ptr()).opr };
          let len = trail.len();
          if len >= 1 && opr.arity() == 1 {
            let mut arg = unsafe { DAG::new((*trail[len - 1].as_ptr()).arg) };
            arg.whnf(defs);
            match arg.head {
              DAGPtr::Lit(link) => {
                let x = unsafe { (*link.as_ptr()).lit.clone() };
                let res = apply_una_op(opr, x);
                if let Some(res) = res {
                  trail.pop();
                  node = DAGPtr::Lit(alloc_val(Lit { lit: res, parents: None }));
                  replace_child(arg.head, node);
                  free_dead_node(arg.head);
                }
                else {
                  break;
                }
              }
              _ => break,
            }
          }
          else if len >= 2 && opr.arity() == 2 {
            let mut arg1 = unsafe {
              DAG::new((*trail[len - 2].as_ptr()).arg)
            };
            let mut arg2 = unsafe {
              DAG::new((*trail[len - 1].as_ptr()).arg)
            };
            arg1.whnf(defs);
            arg2.whnf(defs);
            match (arg1.head, arg2.head) {
              (DAGPtr::Lit(x_link), DAGPtr::Lit(y_link)) => {
                let x = unsafe { (*x_link.as_ptr()).lit.clone() };
                let y = unsafe { (*y_link.as_ptr()).lit.clone() };
                let res = apply_bin_op(opr, y, x);
                if let Some(res) = res {
                  trail.pop();
                  trail.pop();
                  node = DAGPtr::Lit(alloc_val(Lit{ lit: res, parents: None }));
                  replace_child(arg1.head, node);
                  free_dead_node(arg1.head);
                }
                else {
                  break;
                }
              },
              _ => break,
            }
          }
          else {
            break;
          }
        }

        _ => break,
      }
    }
    if trail.is_empty() {
      self.head = node;
    }
    else {
      self.head = DAGPtr::App(trail[0]);
    }
  }

  // Reduce term to its normal form
  pub fn norm(&mut self, defs: &HashMap<Link, Def>) {
    self.whnf(defs);
    let top_node = self.head;
    let mut trail = vec![top_node];
    while let Some(node) = trail.pop() {
      match node {
        DAGPtr::App(link) => unsafe {
          let app = link.as_ptr();
          let mut fun = DAG::new((*app).fun);
          let mut arg = DAG::new((*app).arg);
          fun.whnf(defs);
          arg.whnf(defs);
          trail.push(fun.head);
          trail.push(arg.head);
        },
        DAGPtr::All(link) => unsafe {
          let all = link.as_ptr();
          let mut dom = DAG::new((*all).dom);
          let mut img = DAG::new((*all).img);
          dom.whnf(defs);
          img.whnf(defs);
          trail.push(dom.head);
          trail.push(img.head);
        },
        DAGPtr::Lam(link) => unsafe {
          let lam = link.as_ptr();
          let mut body = DAG::new((*lam).bod);
          body.whnf(defs);
          trail.push(body.head);
        },
        DAGPtr::Slf(link) => unsafe {
          let slf = link.as_ptr();
          let mut body = DAG::new((*slf).bod);
          body.whnf(defs);
          trail.push(body.head);
        },
        DAGPtr::Cse(link) => unsafe {
          let cse = link.as_ptr();
          let mut body = DAG::new((*cse).bod);
          body.whnf(defs);
          trail.push(body.head);
        },
        DAGPtr::Dat(link) => unsafe {
          let dat = link.as_ptr();
          let mut body = DAG::new((*dat).bod);
          body.whnf(defs);
          trail.push(body.head);
        },
        _ => (),
      }
    }
  }
}

#[cfg(test)]
mod test {
  use super::{
    DAG,
  };
  use hashexpr::span::Span;
  use im::HashMap;

  pub fn parse(
    i: &str,
  ) -> nom::IResult<Span, DAG, crate::parse::error::ParseError<Span>> {
    let (i, tree) = crate::parse::term::parse(i)?;
    let (i, _) = nom::character::complete::multispace0(i)?;
    let (i, _) = nom::combinator::eof(i)?;
    let dag = DAG::from_term(&tree);
    Ok((i, dag))
  }

  #[test]
  pub fn parser() {
    fn parse_assert(input: &str) {
      match parse(&input) {
        Ok((_, dag)) => assert_eq!(format!("{}", dag), input),
        Err(_) => panic!("Did not parse."),
      }
    }
    parse_assert("λ x => x");
    parse_assert("λ x y => x y");
    parse_assert("λ y => (λ x => x) y");
    parse_assert("λ y => (λ z => z z) ((λ x => x) y)");
  }

  #[test]
  pub fn reducer() {
    fn norm_assert(input: &str, result: &str) {
      match parse(&input) {
        Ok((_, mut dag)) => {
          dag.norm(&HashMap::new());
          assert_eq!(format!("{}", dag), result)
        }
        Err(_) => panic!("Did not parse."),
      }
    }
    // Already normalized
    norm_assert("λ x => x", "λ x => x");
    norm_assert("λ x y => x y", "λ x y => x y");
    // Not normalized cases
    norm_assert("λ y => (λ x => x) y", "λ y => y");
    norm_assert("λ y => (λ z => z z) ((λ x => x) y)", "λ y => y y");
    // // Church arithmetic
    let zero = "λ s z => z";
    let three = "λ s z => s (s (s z))";
    let four = "λ s z => s (s (s (s z)))";
    let seven = "λ s z => s (s (s (s (s (s (s z))))))";
    let add = "λ m n s z => m s (n s z)";
    let is_three = format!("(({}) ({}) {})", add, zero, three);
    let is_seven = format!("(({}) ({}) {})", add, four, three);
    norm_assert(&is_three, three);
    norm_assert(&is_seven, seven);
    let id = "λ x => x";
    norm_assert(
      &format!("({three}) (({three}) ({id})) ({id})", id = id, three = three),
      id,
    );
    let trm_str =
      &format!("(({n}) (({m}) ({id})) {id})", n = three, m = three, id = id,);
    println!("{}", trm_str);
    let (_, trm) = parse(trm_str).unwrap();
    println!("{:?}", DAG::to_term(&trm));
    // assert_eq!(true, false);
    norm_assert(trm_str, id)
  }
}
