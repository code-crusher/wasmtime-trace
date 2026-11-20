;;! target = "x86_64"
;;! flags = "-W function-references,gc,tail-call"
;;! test = "compile"

;; A function whose only call is a `return_call_ref` is effectively a leaf
;; function: the tail call hands the frame back to the callee before its
;; prologue runs, so no stack-limit check is needed here, and a callee that's
;; statically known to be non-null needs no null check either.

(module
  (type $t (func))
  (global $g (mut (ref $t)) (ref.func $f))
  (global $gn (mut (ref null $t)) (ref.func $f))
  (func $f)

  ;; The callee is a non-nullable `(ref $t)` global: neither a stack-limit
  ;; check nor a null check is needed.
  (func (export "return_call_ref_non_null")
    (return_call_ref $t (global.get $g)))

  ;; A nullable callee still requires the null check, but as this is still
  ;; a tail-only function there's no stack-limit check.
  (func (export "return_call_ref_nullable")
    (return_call_ref $t (global.get $gn)))

  ;; A non-tail `call_ref` to a non-nullable callee skips the null check,
  ;; but the regular call still requires the stack-limit check.
  (func (export "call_ref_non_null")
    (call_ref $t (global.get $g)))
)
;; wasm[0]::function[0]::f:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    %rbp, %rsp
;;       popq    %rbp
;;       retq
;;
;; wasm[0]::function[1]:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    0x30(%rdi), %rsi
;;       movq    %rdi, %r8
;;       movq    8(%rsi), %r10
;;       movq    0x18(%rsi), %rdi
;;       movq    %r8, %rsi
;;       movq    %rbp, %rsp
;;       popq    %rbp
;;       jmpq    *%r10
;;
;; wasm[0]::function[2]:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    0x40(%rdi), %rsi
;;       movq    %rdi, %r8
;;       movq    8(%rsi), %r10
;;       movq    0x18(%rsi), %rdi
;;       movq    %r8, %rsi
;;       movq    %rbp, %rsp
;;       popq    %rbp
;;       jmpq    *%r10
;;
;; wasm[0]::function[3]:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    8(%rdi), %r10
;;       movq    0x18(%r10), %r10
;;       addq    $0x10, %r10
;;       cmpq    %rsp, %r10
;;       ja      0x93
;;   79: movq    0x30(%rdi), %rsi
;;       movq    %rdi, %r9
;;       movq    8(%rsi), %r8
;;       movq    0x18(%rsi), %rdi
;;       movq    %r9, %rsi
;;       callq   *%r8
;;       movq    %rbp, %rsp
;;       popq    %rbp
;;       retq
;;   93: ud2
