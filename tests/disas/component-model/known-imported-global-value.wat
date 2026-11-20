;;! target = "x86_64"
;;! test = "optimize"
;;! filter = "function"
;;! flags = "-C inlining=n -Wconcurrency-support=n"

;; Splitting "known entity import" and "everyone knows this entity's import"
;; into two independent dimensions lets us optimize cases the old, single
;; dimension could not.
;;
;; Module `$M` defines an immutable global and a mutable global and exports
;; both. Module `$N` imports them, and `$N` is only ever instantiated with
;; `$m`'s exports, so `$N`'s imports are always the same defined globals: the
;; first dimension of static knowledge holds. Therefore `$N`'s `get-imm` can
;; inline the immutable global's constant value, which is the same no matter
;; how anyone accesses the global.
;;
;; Module `$O`, however, is instantiated with the exports of two different
;; instances of `$M`, so `$O` cannot know which globals it gets. That makes
;; `$M`'s globals "ambiguous": not every module that may import them
;; statically knows which globals they are. So the second dimension of static
;; knowledge does *not* hold, and neither `$M`'s own accesses of the exported
;; globals nor `$N`'s may use the precise `DefinedGlobal` alias region; they
;; must use the conservative `PublicGlobal` region shared by every global that
;; crosses a module boundary, as `$N`'s `get-mut` does. Inlining the immutable
;; global's constant value is still fine: it is independent of alias regions.

(component
  (core module $M
    (global (export "imm") i32 (i32.const 42))
    (global (export "mut") (mut i32) (i32.const 0))

    (func (export "get-imm") (result i32)
      (global.get 0))
    (func (export "get-mut") (result i32)
      (global.get 1)))
  (core instance $m1 (instantiate $M))
  (core instance $m2 (instantiate $M))

  (core module $N
    (import "" "imm" (global i32))
    (import "" "mut" (global (mut i32)))

    (func (export "get-imm") (result i32)
      (global.get 0))
    (func (export "get-mut") (result i32)
      (global.get 1)))
  (core instance $n (instantiate $N (with "" (instance $m1))))

  (core module $O
    (import "" "imm" (global i32))
    (import "" "mut" (global (mut i32)))

    (func (export "get-imm") (result i32)
      (global.get 0))
    (func (export "get-mut") (result i32)
      (global.get 1)))
  (core instance $o1 (instantiate $O (with "" (instance $m1))))
  (core instance $o2 (instantiate $O (with "" (instance $m2))))
)
;; function u0:0(i64 vmctx, i64) -> i32 tail {
;;     region0 = 123 ""
;;     region1 = 160 ""
;;     gv0 = vmctx
;;     gv1 = load.i64 notrap aligned readonly can_move region0 gv0+8
;;     gv2 = load.i64 notrap aligned region1 gv1+24
;;     stack_limit = gv2
;;
;;                                 block0(v0: i64, v1: i64):
;; @0055                               jump block1
;;
;;                                 block1:
;; @0053                               v2 = iconst.i32 42
;; @0055                               return v2  ; v2 = 42
;; }
;;
;; function u0:1(i64 vmctx, i64) -> i32 tail {
;;     region0 = 123 ""
;;     region1 = 160 ""
;;     region2 = 87 ""
;;     gv0 = vmctx
;;     gv1 = load.i64 notrap aligned readonly can_move region0 gv0+8
;;     gv2 = load.i64 notrap aligned region1 gv1+24
;;     stack_limit = gv2
;;
;;                                 block0(v0: i64, v1: i64):
;; @0058                               v2 = load.i32 notrap aligned region2 v0+64
;; @005a                               jump block1
;;
;;                                 block1:
;; @005a                               return v2
;; }
;;
;; function u1:0(i64 vmctx, i64) -> i32 tail {
;;     region0 = 123 ""
;;     region1 = 160 ""
;;     gv0 = vmctx
;;     gv1 = load.i64 notrap aligned readonly can_move region0 gv0+8
;;     gv2 = load.i64 notrap aligned region1 gv1+24
;;     stack_limit = gv2
;;
;;                                 block0(v0: i64, v1: i64):
;; @00b6                               jump block1
;;
;;                                 block1:
;; @00b4                               v2 = iconst.i32 42
;; @00b6                               return v2  ; v2 = 42
;; }
;;
;; function u1:1(i64 vmctx, i64) -> i32 tail {
;;     region0 = 123 ""
;;     region1 = 160 ""
;;     region2 = 57 ""
;;     region3 = 87 ""
;;     gv0 = vmctx
;;     gv1 = load.i64 notrap aligned readonly can_move region0 gv0+8
;;     gv2 = load.i64 notrap aligned region1 gv1+24
;;     stack_limit = gv2
;;
;;                                 block0(v0: i64, v1: i64):
;; @00b9                               v2 = load.i64 notrap aligned readonly can_move region2 v0+72
;; @00b9                               v3 = load.i32 notrap aligned region3 v2
;; @00bb                               jump block1
;;
;;                                 block1:
;; @00bb                               return v3
;; }
;;
;; function u2:0(i64 vmctx, i64) -> i32 tail {
;;     region0 = 123 ""
;;     region1 = 160 ""
;;     region2 = 57 ""
;;     region3 = 87 ""
;;     gv0 = vmctx
;;     gv1 = load.i64 notrap aligned readonly can_move region0 gv0+8
;;     gv2 = load.i64 notrap aligned region1 gv1+24
;;     stack_limit = gv2
;;
;;                                 block0(v0: i64, v1: i64):
;; @0115                               v2 = load.i64 notrap aligned readonly can_move region2 v0+48
;; @0115                               v3 = load.i32 notrap aligned region3 v2
;; @0117                               jump block1
;;
;;                                 block1:
;; @0117                               return v3
;; }
;;
;; function u2:1(i64 vmctx, i64) -> i32 tail {
;;     region0 = 123 ""
;;     region1 = 160 ""
;;     region2 = 57 ""
;;     region3 = 87 ""
;;     gv0 = vmctx
;;     gv1 = load.i64 notrap aligned readonly can_move region0 gv0+8
;;     gv2 = load.i64 notrap aligned region1 gv1+24
;;     stack_limit = gv2
;;
;;                                 block0(v0: i64, v1: i64):
;; @011a                               v2 = load.i64 notrap aligned readonly can_move region2 v0+72
;; @011a                               v3 = load.i32 notrap aligned region3 v2
;; @011c                               jump block1
;;
;;                                 block1:
;; @011c                               return v3
;; }
