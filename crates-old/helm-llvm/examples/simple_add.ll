; Simple addition example for helm-llvm
; Demonstrates basic integer arithmetic

define i32 @simple_add(i32 %a, i32 %b) {
entry:
  %result = add i32 %a, %b
  ret i32 %result
}

define void @main() {
entry:
  %x = add i32 10, 20
  %y = add i32 %x, 5
  ret void
}
