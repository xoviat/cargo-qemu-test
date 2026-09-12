/* defmt-macros emits one input section per interned string, named
   .defmt.<tag>.<json-metadata> (1 byte each). Nothing in firmware references
   them, so without this script rust-lld --gc-sections garbage-collects them
   (only .defmt.end survives via SHF_GNU_RETAIN) and the survivors stay as
   orphans under their long names. defmt-decoder looks up a section exactly
   named ".defmt" and reads the JSON symbol names inside it, so merge + keep: */
SECTIONS {
    .defmt : {
        KEEP(*(.defmt .defmt.*));
    }
}
INSERT AFTER .text;
