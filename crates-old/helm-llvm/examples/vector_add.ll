; Vector addition example
; Demonstrates SIMD-style operations

define void @vector_add(float* %A, float* %B, float* %C, i32 %N) {
entry:
  br label %loop

loop:
  %i = phi i32 [ 0, %entry ], [ %i_next, %loop ]
  
  ; Load A[i]
  %a_ptr = getelementptr float, float* %A, i32 %i
  %a_val = load float, float* %a_ptr
  
  ; Load B[i]
  %b_ptr = getelementptr float, float* %B, i32 %i
  %b_val = load float, float* %b_ptr
  
  ; C[i] = A[i] + B[i]
  %sum = fadd float %a_val, %b_val
  
  ; Store C[i]
  %c_ptr = getelementptr float, float* %C, i32 %i
  store float %sum, float* %c_ptr
  
  ; Loop increment
  %i_next = add i32 %i, 1
  %cmp = icmp slt i32 %i_next, %N
  br i1 %cmp, label %loop, label %exit

exit:
  ret void
}

define void @main() {
entry:
  ret void
}
