; Matrix multiply kernel example
; Demonstrates floating-point operations and memory access patterns

define void @matmul(float* %A, float* %B, float* %C, i32 %N) {
entry:
  br label %outer_loop

outer_loop:
  %i = phi i32 [ 0, %entry ], [ %i_next, %outer_latch ]
  %i_cmp = icmp slt i32 %i, %N
  br i1 %i_cmp, label %middle_loop, label %exit

middle_loop:
  %j = phi i32 [ 0, %outer_loop ], [ %j_next, %middle_latch ]
  %j_cmp = icmp slt i32 %j, %N
  br i1 %j_cmp, label %inner_loop, label %outer_latch

inner_loop:
  %k = phi i32 [ 0, %middle_loop ], [ %k_next, %inner_loop ]
  %acc = phi float [ 0.0, %middle_loop ], [ %new_acc, %inner_loop ]
  
  ; Calculate A[i*N + k]
  %a_row_offset = mul i32 %i, %N
  %a_idx = add i32 %a_row_offset, %k
  %a_ptr = getelementptr float, float* %A, i32 %a_idx
  %a_val = load float, float* %a_ptr
  
  ; Calculate B[k*N + j]
  %b_row_offset = mul i32 %k, %N
  %b_idx = add i32 %b_row_offset, %j
  %b_ptr = getelementptr float, float* %B, i32 %b_idx
  %b_val = load float, float* %b_ptr
  
  ; Accumulate: acc += A[i,k] * B[k,j]
  %prod = fmul float %a_val, %b_val
  %new_acc = fadd float %acc, %prod
  
  %k_next = add i32 %k, 1
  %k_cmp = icmp slt i32 %k_next, %N
  br i1 %k_cmp, label %inner_loop, label %store_result

store_result:
  ; Store C[i*N + j] = acc
  %c_row_offset = mul i32 %i, %N
  %c_idx = add i32 %c_row_offset, %j
  %c_ptr = getelementptr float, float* %C, i32 %c_idx
  store float %new_acc, float* %c_ptr
  br label %middle_latch

middle_latch:
  %j_next = add i32 %j, 1
  br label %middle_loop

outer_latch:
  %i_next = add i32 %i, 1
  br label %outer_loop

exit:
  ret void
}
