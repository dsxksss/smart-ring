ENTRY(r08_custom_control)

SECTIONS
{
  /* Start 4-byte aligned inside the zero run so Thumb LDR literals align. */
  .patch 0x00849B08 : SUBALIGN(2)
  {
    KEEP(*(.patch))
  }
  /DISCARD/ :
  {
    *(.ARM.attributes)
    *(.comment)
  }
}
