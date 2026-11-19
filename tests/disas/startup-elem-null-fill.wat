;;! target = 'x86_64'
;;! test = 'optimize'
;;! filter = 'module_start'
;;! flags = '-Wgc -Wfunction-references -O table-lazy-init=no'

;; Element segments whose elements are all `ref.null` are compiled to a single
;; fill loop (like `table.fill`) rather than a `table.set` per element. This
;; keeps the size of the module startup function independent of the size of the
;; element segment which matters for huge null-only segments. (With lazy table
;; initialization enabled the segment is instead folded into the precomputed
;; table image and no startup function is needed at all, so lazy table init is
;; disabled here to exercise the fill codegen.)

(module
  (table 10 (ref null func))
  (elem (i32.const 1) funcref (ref.null func) (ref.null func) (ref.null func))
)
;; function u2415919104:1(i64 vmctx, i64, i64, i64) -> i8 system_v {
;;     region0 = 123 ""
;;     region1 = 231 ""
;;     region2 = 243 ""
;;     region3 = 209 ""
;;     region4 = 68 ""
;;     region5 = 30 ""
;;     sig0 = (i64 vmctx, i64) tail
;;     fn0 = colocated u2415919104:0 sig0
;;
;; block0(v0: i64, v1: i64, v2: i64, v3: i64):
;;     jump block1
;;
;; block1:
;;     v7 = get_frame_pointer.i64 
;;     v6 = load.i64 notrap aligned readonly can_move region0 v0+8
;;     store notrap aligned region1 v7, v6+72
;;     v8 = get_stack_pointer.i64 
;;     store notrap aligned region2 v8, v6+64
;;     v9 = get_exception_handler_address.i64 block1, 0
;;     store notrap aligned region3 v9, v6+80
;;     try_call fn0(v0, v1), sig0, block2, [ default: block3 ]
;;
;; block2:
;;     v14 = iconst.i8 1
;;     return v14  ; v14 = 1
;;
;; block3:
;;     v11 = iconst.i64 1
;;     store notrap aligned region4 v11, v6+136  ; v11 = 1
;;     v4 = iconst.i64 0
;;     store notrap aligned region5 v4, v6+152  ; v4 = 0
;;     v13 = iconst.i8 0
;;     return v13  ; v13 = 0
;; }
;;
;; function u2415919104:0(i64 vmctx, i64) tail {
;;     region0 = 93 ""
;;     region1 = 211 ""
;;     region2 = 117 ""
;;
;; block0(v0: i64, v1: i64):
;;     v4 = load.i64 notrap aligned region1 v0+56
;;     v5 = ireduce.i32 v4
;;     v6 = uextend.i64 v5
;;     v52 = iconst.i64 4
;;     v58 = icmp ult v6, v52  ; v52 = 4
;;     trapnz v58, user6
;;     v13 = load.i64 notrap aligned region0 v0+48
;;     v15 = iconst.i64 8
;;     v17 = iadd v13, v15  ; v15 = 8
;;     v45 = iconst.i64 0
;;     v67 = iconst.i64 24
;;     v97 = iadd v13, v67  ; v67 = 24
;;     jump block1(v17)
;;
;; block1(v39: i64):
;;     v118 = iconst.i64 0
;;     store notrap aligned region2 v118, v39  ; v118 = 0
;;     v119 = iadd.i64 v13, v67  ; v67 = 24
;;     v120 = icmp eq v39, v119
;;     v121 = iconst.i64 8
;;     v122 = iadd v39, v121  ; v121 = 8
;;     brif v120, block2, block1(v122)
;;
;; block2:
;;     return
;; }
