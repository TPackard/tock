/* Memory Space Definitions, 256K flash, 64K ram */
MEMORY
{
  rom (rx)  : ORIGIN = 0x01000000, LENGTH = 192K
  prog (rx) : ORIGIN = 0x01030000, LENGTH = 64K
  ram (rwx) : ORIGIN = 0x21000000, LENGTH = 64K
}

MPU_MIN_ALIGN = 8K;
