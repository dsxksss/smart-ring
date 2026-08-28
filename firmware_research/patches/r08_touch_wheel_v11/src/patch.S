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
 * Stock vertical gestures contain a button-up calibration sample in the
 * opposite direction, followed by button-down samples in the intended
 * direction.  Emit wheel input only while button 1 is held and only for the
 * two larger-magnitude Y samples (abs(Y) >= 16).  This removes the leading
 * reversal and reduces one gesture from roughly six notches to two.  Always
 * clear buttons/X/Y before resuming the stock HID packing path.
 */
.global r08_touch_wheel_v11_thunk
.type r08_touch_wheel_v11_thunk, %function
.thumb_func
r08_touch_wheel_v11_thunk:
    cmp  r0, #1
    bne  .Lzero
    cmp  r2, #16
    bge  .Lpositive
    movs r4, #16
    negs r4, r4
    cmp  r2, r4
    ble  .Lnegative
    b    .Lzero
.Lnegative:
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

.size r08_touch_wheel_v11_thunk, . - r08_touch_wheel_v11_thunk
