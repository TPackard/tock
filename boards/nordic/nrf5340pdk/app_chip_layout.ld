/* Memory Space Definitions, 1M flash, 512K ram */
MEMORY
{
  rom (rx)  : ORIGIN = 0x00000000, LENGTH = 192K
  prog (rx) : ORIGIN = 0x00030000, LENGTH = 832K
  ram (rwx) : ORIGIN = 0x20000000, LENGTH = 512K
}

MPU_MIN_ALIGN = 8K;
