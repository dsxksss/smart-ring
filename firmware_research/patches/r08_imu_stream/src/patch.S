.syntax unified
.cpu cortex-m0plus
.thumb

.section .patch, "ax", %progbits
.balign 2

/*
 * Hook target for stock A1 invalid-subcommand path at 0x008280F6.
 * Entry invariants recovered from the stock handler:
 *   r5 = validated 16-byte command, r6 = 0, r7 = 0x00209CB8.
 * For commands other than A1 09 xx, reproduce the two overwritten STRB
 * instructions exactly and return to 0x008280FA.
 */
.global r08_custom_control
.type r08_custom_control, %function
.thumb_func
r08_custom_control:
    ldrb r0, [r5, #1]
    cmp  r0, #9
    beq  1f
    strb r6, [r7, #0]
    strb r6, [r7, #1]
    bx   lr
1:
    /* Two saved words keep the inherited 8-byte AAPCS stack alignment. */
    push {r0, lr}
    ldrb r0, [r5, #2]
    cmp  r0, #1
    beq  .Lstart
    bl   r08_custom_stop
    b    .Lcontrol_done

.Lstart:
    /* Always tear down an earlier timer before changing its callback. */
    bl   r08_custom_stop

    /* Start the hard-stop timer first; do not power the IMU if creation fails. */
    ldr  r0, .La1_timer
    ldr  r1, .Ltimer_callback
    movs r2, #100
    movs r3, #1
    ldr  r4, .Lfn_timer_start
    blx  r4
    cmp  r0, #0
    beq  .Lcontrol_done

    ldr  r3, .Lfn_sensor_25hz
    blx  r3

    /* Enable the stock FIFO drain path and remember its current write index. */
    ldr  r4, .Lring_state
    movs r0, #1
    strb r0, [r4, #1]
    ldrh r1, [r4, #8]

    ldr  r4, .La1_state
    movs r0, #9
    strb r0, [r4, #0]
    strh r1, [r4, #4]
    /* stop cleared sequence + watchdog ticks together with the mode word */

.Lcontrol_done:
    pop  {r0, r4}
    mov  lr, r4
    /* Unique diagnostic status: A1 FE proves this patched hook executed. */
    movs r1, #0xFE
    ldr  r0, .La1_epilogue
    bx   r0

.global r08_custom_stop
.type r08_custom_stop, %function
.thumb_func
r08_custom_stop:
    push {r4, lr}
    ldr  r4, .La1_state
    movs r0, #0
    str  r0, [r4, #0]
    str  r0, [r4, #4]

    ldr  r0, .La1_timer
    ldr  r3, .Lfn_timer_stop
    blx  r3
    ldr  r3, .Lfn_fifo_stop
    blx  r3
    ldr  r3, .Lfn_sensor_standby
    blx  r3

    movs r0, #0
    ldr  r4, .Lring_state
    strb r0, [r4, #1]
    pop  {r4, pc}

/* Repeating 100 ms callback installed by r08_custom_control. */
.global r08_stream_tick
.type r08_stream_tick, %function
.thumb_func
r08_stream_tick:
    push {r4, r5, r6, r7, lr}
    /* 20-byte PUSH + 28-byte local frame remains 8-byte aligned at calls. */
    sub  sp, #28
    ldr  r4, .La1_state
    ldrb r0, [r4, #0]
    cmp  r0, #9
    bne  .Ltick_done

    /* Fail closed before touching the FIFO when the NUS link is gone. */
    ldr  r7, .Lfn_connected
    blx  r7
    cmp  r0, #0
    beq  .Lhard_stop

    /* Zero the packet first so a temporarily empty FIFO produces safe zeros. */
    movs r0, #0
    str  r0, [sp, #0]
    str  r0, [sp, #4]
    str  r0, [sp, #8]
    str  r0, [sp, #12]

    /* sp+4/6/8 are the signed XYZ outputs before byte-order swapping. */
    mov  r0, sp
    adds r0, #4
    mov  r1, sp
    adds r1, #6
    mov  r2, sp
    adds r2, #8
    movs r3, #1
    ldr  r7, .Lfn_ring_latest
    blx  r7

    /* The stock producer advances this byte count by 6 for every fresh XYZ. */
    ldr  r6, .Lring_state
    ldrh r6, [r6, #8]
    ldrh r0, [r4, #4]
    cmp  r0, r6
    beq  .Lstale
    strh r6, [r4, #4]
    movs r6, #1
    b    .Lflags_ready
.Lstale:
    movs r6, #2
.Lflags_ready:

    mov  r5, sp
    movs r0, #0xA2
    strb r0, [r5, #0]
    movs r0, #0x10
    strb r0, [r5, #1]
    ldrb r0, [r4, #2]
    strb r0, [r5, #2]
    strb r6, [r5, #3]
    /* XYZ remain native little-endian, matching the RTL8762E stores. */

    mov  r0, sp
    movs r1, #15
    ldr  r7, .Lfn_checksum
    blx  r7
    mov  r5, sp
    strb r0, [r5, #15]
    mov  r0, sp
    ldr  r7, .Lfn_notify
    blx  r7

    cmp  r6, #1
    bne  .Lhard_stop

    ldrb r0, [r4, #2]
    adds r0, #1
    strb r0, [r4, #2]
    ldrb r0, [r4, #3]
    adds r0, #1
    strb r0, [r4, #3]
    cmp  r0, #120          /* 12 s hard stop; host refreshes every 8 s */
    blo  .Ltick_done
.Lhard_stop:
    bl   r08_custom_stop

.Ltick_done:
    add  sp, #28
    pop  {r4, r5, r6, r7, pc}

.balign 4
.La1_state:           .word 0x00209CB8
.La1_timer:           .word 0x00209CC8
.Lring_state:         .word 0x0020BFA0
.La1_epilogue:        .word 0x00828115
/* .thumb_func already gives the relocation a Thumb-state low bit. */
.Ltimer_callback:     .word r08_stream_tick
.Lfn_timer_start:     .word 0x00829F19
.Lfn_timer_stop:      .word 0x00829F45
.Lfn_sensor_25hz:     .word 0x00832D07
.Lfn_fifo_stop:       .word 0x00832CBD
.Lfn_sensor_standby:  .word 0x008335FD
.Lfn_ring_latest:     .word 0x0083394F
.Lfn_checksum:        .word 0x0082AC01
.Lfn_connected:       .word 0x0082DCFF
.Lfn_notify:          .word 0x0082E975

.size r08_custom_control, . - r08_custom_control
