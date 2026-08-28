.syntax unified
.cpu cortex-m0plus
.thumb

.section .patch, "ax", %progbits
.balign 2

/*
 * Called from the stock HID mouse motion helper after its connection guard.
 *
 * Entry registers recovered from the stock queue consumer:
 *   r0 = buttons, r1 = signed X, r2 = signed Y, r3 = signed wheel.
 * The stock helper has already saved r0-r4 and lr on its own stack.
 *
 * Convert only the sign of Y to one wheel notch, and force buttons/X/Y to
 * zero.  The caller resumes at the stock report-packing instructions, so the
 * original attribute index, connection guard and server_send_data path remain
 * intact without ever emitting pointer movement or mouse buttons.
 */
.global r08_touch_wheel_thunk
.type r08_touch_wheel_thunk, %function
.thumb_func
r08_touch_wheel_thunk:
    cmp  r2, #0
    beq  .Lzero
    bgt  .Lpositive
    movs r3, #1
    negs r3, r3
    b    .Lready
.Lpositive:
    movs r3, #1
    b    .Lready
.Lzero:
    movs r3, #0
.Lready:
    movs r0, #0
    movs r1, #0
    movs r2, #0
    mov  r4, sp
    strb r0, [r4, #8]
    bx   lr

.size r08_touch_wheel_thunk, . - r08_touch_wheel_thunk
