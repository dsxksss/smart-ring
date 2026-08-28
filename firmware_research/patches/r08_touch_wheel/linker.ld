MEMORY
{
  PATCH (rx) : ORIGIN = 0x00829FD6, LENGTH = 0x4A
}

SECTIONS
{
  .patch ORIGIN(PATCH) :
  {
    KEEP(*(.patch))
  } > PATCH
}
