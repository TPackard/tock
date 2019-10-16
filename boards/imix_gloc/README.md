imix\_gloc: Platform-Specific Instructions
==========================================

This board file is a variant of the standard imix kernel that allows the
complete GLOC test suite to be run. Due to the GLOC requiring the use of pins
allocated to other components, this board file disables certain components. In
particular, all references to the RF233 radio have been removed. For more
information on running Tock on the imix, see `README.md` in the `boards/imix`
directory.
