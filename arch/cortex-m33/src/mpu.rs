//! Implementation of the memory protection unit for the Cortex-M33.

use core::cmp;
use core::fmt;
use kernel;
use kernel::common::registers::{register_bitfields, FieldValue, ReadOnly, ReadWrite};
use kernel::common::StaticRef;
use kernel::mpu;

/// MPU Registers described in section 4.5 of
/// <http://infocenter.arm.com/help/topic/com.arm.doc.100235_0003_00_en/arm_cortex_m33_dgug_100235_0003_00_en.pdf>
#[repr(C)]
pub struct MpuRegisters {
    /// Indicates whether the MPU is present and, if so, how many regions it
    /// supports.
    pub mpu_type: ReadOnly<u32, Type::Register>,

    /// The control register:
    ///   * Enables the MPU (bit 0).
    ///   * Enables MPU in hard-fault, non-maskable interrupt (NMI).
    ///   * Enables the default memory map background region in privileged mode.
    pub ctrl: ReadWrite<u32, Control::Register>,

    /// Selects the region number (zero-indexed) referenced by the region base
    /// address and region attribute and size registers.
    pub rnr: ReadWrite<u32, RegionNumber::Register>,

    /// Defines the base address of the currently selected MPU region. Also defines shareability,
    /// access permissions, and execute permissions.
    pub rbar: ReadWrite<u32, RegionBaseAddress::Register>,

    /// Defines the region limit address of the selected MPU region. Also includes attribute index
    /// and region enable bit.
    pub rlar: ReadWrite<u32, RegionLimitAddress::Register>,

    _reserved: [u32; 0x20],

    /// Defines the attributes for all of the memory regions.
    pub mair0: ReadWrite<u32, RegionAttributes::Register>,
    pub mair1: ReadWrite<u32, RegionAttributes::Register>,
}

register_bitfields![u32,
    Type [
        /// The number of data regions supported. If this field reads-as-zero the
        /// processor does not implement an MPU
        DREGION OFFSET(8) NUMBITS(8) [],
        /// Indicates whether the processor support unified (0) or separate
        /// (1) instruction and data regions. Always reads 0 on the
        /// Cortex-M33.
        SEPARATE OFFSET(0) NUMBITS(1) [
            Unified = 0,
            Separate = 1
        ]
    ],

    Control [
        /// Enables privileged software access to the default
        /// memory map
        PRIVDEFENA OFFSET(2) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ],
        /// Enables the operation of MPU during hard fault, NMI,
        /// and FAULTMASK handlers
        HFNMIENA OFFSET(1) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ],
        /// Enables the MPU
        ENABLE OFFSET(0) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ]
    ],

    RegionNumber [
        /// Region indicating the MPU region referenced by the MPU_RBAR and
        /// MPU_RASR registers. Range 0-7 corresponding to the MPU regions.
        REGION OFFSET(0) NUMBITS(8) []
    ],

    RegionBaseAddress [
        /// Base address of the currently selected MPU region.
        BASE OFFSET(5) NUMBITS(27) [],
        /// Shareability domain of the currently selected MPU region for normal memory.
        /// Ignored for device memory.
        SH OFFSET(3) NUMBITS(2) [
            NonShareable = 0,
            OuterShareable = 2,
            InnerShareable = 3
        ],
        /// Accesss permissions for currently selected MPU region.
        AP OFFSET(1) NUMBITS(2) [
            RWPriv = 0,
            RWAny = 1,
            ROPriv = 2,
            ROAny = 3
        ],
        /// Whether or not code may be executed within the currently selected MPU region.
        XN OFFSET(0) NUMBITS(1) [
            ExecuteIfReadable = 0,
            ExecuteNever = 1
        ]
    ],

    RegionLimitAddress [
        /// Limit address of the currently selected MPU region.
        LIMIT OFFSET(5) NUMBITS(27) [],
        /// Index of attributes in MAIR0 and MAIR1 for currently selected MPU region.
        AttrIndx OFFSET(1) NUMBITS(3) [],
        ///Enables the currently selected MPU region.
        EN OFFSET(0) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ]
    ],

    RegionAttributes [
        /// Attributes for MPU regions. See section 4.5.17 in ARM Cortex-M33 Devices Generic User
        /// Guide for more detail.
        Attr3 OFFSET(24) NUMBITS(8) [],
        Attr2 OFFSET(16) NUMBITS(8) [],
        Attr1 OFFSET(8) NUMBITS(8) [],
        Attr0 OFFSET(0) NUMBITS(8) []
    ]
];

const MPU_BASE_ADDRESS: StaticRef<MpuRegisters> =
    unsafe { StaticRef::new(0xE000ED90 as *const MpuRegisters) };

/// Constructor field is private to limit who can create a new MPU
pub struct MPU(StaticRef<MpuRegisters>);

impl MPU {
    pub const unsafe fn new() -> MPU {
        MPU(MPU_BASE_ADDRESS)
    }
}

/// Struct storing region configuration for the Cortex-M MPU.
#[derive(Copy, Clone)]
pub struct CortexMConfig {
    regions: [CortexMRegion; 8],
}

const APP_MEMORY_REGION_NUM: usize = 0;

impl Default for CortexMConfig {
    fn default() -> CortexMConfig {
        CortexMConfig {
            regions: [
                CortexMRegion::empty(),
                CortexMRegion::empty(),
                CortexMRegion::empty(),
                CortexMRegion::empty(),
                CortexMRegion::empty(),
                CortexMRegion::empty(),
                CortexMRegion::empty(),
                CortexMRegion::empty(),
            ],
        }
    }
}

impl fmt::Display for CortexMConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\r\n Cortex-M33 MPU")?;
        for (i, region) in self.regions.iter().enumerate() {
            if let Some(location) = region.location() {
                let access_bits = region.base_address().read(RegionBaseAddress::AP);
                let access_str = match access_bits {
                    0b00 => "ReadWritePrivileged",
                    0b01 => "ReadWriteAny",
                    0b10 => "ReadOnlyPrivileged",
                    0b11 => "ReadOnlyAny",
                    _ => "ERR",
                };
                let start = location.0 as usize;
                let size = location.1 as usize;
                write!(
                    f,
                    "\
                     \r\n  Region {}: [{:#010X}:{:#010X}], length: {} bytes; {} ({:#x})",
                    i,
                    start,
                    start + size,
                    size,
                    access_str,
                    access_bits,
                )?;
            } else {
                write!(f, "\r\n  Region {}: Unused", i)?;
            }
        }
        write!(f, "\r\n")
    }
}

impl CortexMConfig {
    fn unused_region_number(&self) -> Option<usize> {
        for (number, region) in self.regions.iter().enumerate() {
            if number == APP_MEMORY_REGION_NUM {
                continue;
            }
            if let None = region.location() {
                return Some(number);
            }
        }
        None
    }
}

/// Struct storing configuration for a Cortex-M MPU region.
#[derive(Copy, Clone)]
pub struct CortexMRegion {
    location: Option<(*const u8, usize)>,
    base_address: FieldValue<u32, RegionBaseAddress::Register>,
    limit_address: FieldValue<u32, RegionLimitAddress::Register>,
}

impl CortexMRegion {
    fn new(
        start: *const u8,
        allocated_size: usize,
        protected_size: usize,
        permissions: mpu::Permissions,
    ) -> CortexMRegion {
        // Determine access and execute permissions
        let (access, execute) = match permissions {
            mpu::Permissions::ReadWriteExecute => (
                RegionBaseAddress::AP::RWAny,
                RegionBaseAddress::XN::ExecuteIfReadable,
            ),
            mpu::Permissions::ReadWriteOnly => (
                RegionBaseAddress::AP::RWAny,
                RegionBaseAddress::XN::ExecuteNever,
            ),
            mpu::Permissions::ReadExecuteOnly => (
                RegionBaseAddress::AP::ROAny,
                RegionBaseAddress::XN::ExecuteIfReadable,
            ),
            mpu::Permissions::ReadOnly => (
                RegionBaseAddress::AP::ROAny,
                RegionBaseAddress::XN::ExecuteNever,
            ),
            mpu::Permissions::ExecuteOnly => (
                // Must be readable in order to execute, so this is the same as ReadExecuteOnly.
                RegionBaseAddress::AP::ROAny,
                RegionBaseAddress::XN::ExecuteIfReadable,
            ),
        };

        // Base address register
        let base_address = RegionBaseAddress::BASE.val((start as u32) >> 5)
            + access
            + execute;

        // Limit address register
        let limit_address = (start as usize) + protected_size - 1;
        let limit_address = RegionLimitAddress::LIMIT.val((limit_address as u32) >> 5)
            + RegionLimitAddress::EN::Enable;

        CortexMRegion {
            location: Some((start, allocated_size)),
            base_address,
            limit_address,
        }
    }

    fn empty() -> CortexMRegion {
        CortexMRegion {
            location: None,
            base_address: RegionBaseAddress::BASE.val(0),
            limit_address: RegionLimitAddress::EN::CLEAR,
        }
    }

    fn location(&self) -> Option<(*const u8, usize)> {
        self.location
    }

    fn base_address(&self) -> FieldValue<u32, RegionBaseAddress::Register> {
        self.base_address
    }

    fn limit_address(&self) -> FieldValue<u32, RegionLimitAddress::Register> {
        self.limit_address
    }

    fn overlaps(&self, other_start: *const u8, other_size: usize) -> bool {
        let other_start = other_start as usize;
        let other_end = other_start + other_size;

        let (region_start, region_end) = match self.location {
            Some((region_start, region_size)) => {
                let region_start = region_start as usize;
                let region_end = region_start + region_size;
                (region_start, region_end)
            }
            None => return false,
        };

        if region_start < other_end && other_start < region_end {
            true
        } else {
            false
        }
    }
}

impl kernel::mpu::MPU for MPU {
    type MpuConfig = CortexMConfig;

    fn enable_mpu(&self) {
        let regs = &*self.0;

        // Enable the MPU, disable it during HardFault/NMI handlers, and allow
        // privileged code access to all unprotected memory.
        regs.ctrl
            .write(Control::ENABLE::Enable + Control::HFNMIENA::Disable + Control::PRIVDEFENA::Enable);
    }

    fn disable_mpu(&self) {
        let regs = &*self.0;
        regs.ctrl.write(Control::ENABLE::Disable);
    }

    fn number_total_regions(&self) -> usize {
        let regs = &*self.0;
        regs.mpu_type.read(Type::DREGION) as usize
    }

    fn allocate_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_region_size: usize,
        permissions: mpu::Permissions,
        config: &mut Self::MpuConfig,
    ) -> Option<mpu::Region> {
        // Check that no previously allocated regions overlap the unallocated memory.
        for region in config.regions.iter() {
            if region.overlaps(unallocated_memory_start, unallocated_memory_size) {
                return None;
            }
        }

        // Logical region
        let region_num = config.unused_region_number()?;
        let mut start = unallocated_memory_start as usize;
        let mut size = min_region_size;

        // Regions must be 32 byte aligned.
        if start % 32 != 0 {
            start += 32 - (start % 32);
        }
        if size % 32 != 0 {
            size += 32 - (size % 32);
        }

        // Check that our logical region fits in memory.
        if start + size > (unallocated_memory_start as usize) + unallocated_memory_size {
            return None;
        }

        let region = CortexMRegion::new(
            start as *const u8,
            size,
            size,
            permissions,
        );

        config.regions[region_num] = region;

        Some(mpu::Region::new(start as *const u8, size))
    }

    fn allocate_app_memory_region(
        &self,
        unallocated_memory_start: *const u8,
        unallocated_memory_size: usize,
        min_memory_size: usize,
        initial_app_memory_size: usize,
        initial_kernel_memory_size: usize,
        permissions: mpu::Permissions,
        config: &mut Self::MpuConfig,
    ) -> Option<(*const u8, usize)> {
        // Check that no previously allocated regions overlap the unallocated memory.
        for region in config.regions.iter() {
            if region.overlaps(unallocated_memory_start, unallocated_memory_size) {
                return None;
            }
        }

        // The region should start as close as possible to the start of the unallocated memory.
        let mut region_start = unallocated_memory_start as usize;
        let mut app_memory_size = initial_app_memory_size;

        // Regions must be 32 byte aligned.
        if region_start % 32 != 0 {
            region_start += 32 - (region_start % 32);
        }
        if app_memory_size % 32 != 0 {
            app_memory_size += 32 - (app_memory_size % 32);
        }

        // Make sure there is enough memory for app memory and kernel memory. Allocate a bit more
        // space so that there's room for expanding the app and kernel memory regions.
        let combined_memory_size = app_memory_size + initial_kernel_memory_size;
        let mut region_size = cmp::max(
            min_memory_size,
            combined_memory_size + combined_memory_size / 4,
        );
        if region_size % 32 != 0 {
            region_size += 32 - (region_size % 32);
        }

        // Make sure the region fits in the unallocated memory.
        if region_start + region_size
            > (unallocated_memory_start as usize) + unallocated_memory_size
        {
            return None;
        }

        let region = CortexMRegion::new(
            region_start as *const u8,
            region_size,
            app_memory_size,
            permissions,
        );

        config.regions[APP_MEMORY_REGION_NUM] = region;

        Some((region_start as *const u8, region_size))
    }

    fn update_app_memory_region(
        &self,
        app_memory_break: *const u8,
        kernel_memory_break: *const u8,
        permissions: mpu::Permissions,
        config: &mut Self::MpuConfig,
    ) -> Result<(), ()> {
        let (region_start, region_size) = match config.regions[APP_MEMORY_REGION_NUM].location() {
            Some((start, size)) => (start as usize, size),
            None => {
                // Error: Process tried to update app memory MPU region before it was created.
                return Err(());
            }
        };

        let mut app_memory_break = app_memory_break as usize;
        let kernel_memory_break = kernel_memory_break as usize;

        // Regions must be 32 byte aligned.
        if app_memory_break % 32 != 0 {
            app_memory_break += 32 - (app_memory_break % 32);
        }

        // Out of memory
        if app_memory_break > kernel_memory_break {
            return Err(());
        }

        let region = CortexMRegion::new(
            region_start as *const u8,
            region_size,
            app_memory_break - region_start,
            permissions,
        );

        config.regions[APP_MEMORY_REGION_NUM] = region;

        Ok(())
    }

    fn configure_mpu(&self, config: &Self::MpuConfig) {
        let regs = &*self.0;

        // All regions use Device-nGnRnE attributes by default.
        regs.mair0.write(RegionAttributes::Attr0.val(0b0000_0000));

        // Set MPU regions
        for (number, region) in config.regions.iter().enumerate() {
            regs.rnr.write(RegionNumber::REGION.val(number as u32));
            regs.rbar.write(region.base_address());
            regs.rlar.write(region.limit_address());
        }
    }
}
