# Split RAR archives made by RAR

These sets are test fixtures of the `rars` crate (version 0.12.1,
Apache-2.0, <https://crates.io/crates/rars>), copied unchanged from its
`tests/fixtures` (`rar50/`, `rar15_40/rar300/` and `rar13/`). They are there
as archives written by RAR itself (RAR 1.x, RAR 3.00 and WinRAR 7), not by
that crate's own writer, so reading them checks more than a round trip.

| Set | Format | Holds | Password |
| --- | --- | --- | --- |
| `multivol.partN.rar` | RAR 5 | `random_4k.bin` | |
| `encrypted_multivol.partN.rar` | RAR 5 | `random_4k.bin` | `password` |
| `header_encrypted_stored_multivol.partN.rar` | RAR 5, names encrypted, stored | `stored_4k.bin` | `password` |
| `solid_multivol.partNN.rar` | RAR 5, solid | `tiny.txt`, `bigtext_64k.bin` | |
| `multivol_oldnaming_rar300.rar`, `.r00` | RAR 3 | `bigtext_64k.bin` | |
| `compressed_multivol_prng_rar300.rar`, `.r00`... | RAR 3 | `cvolume.bin` | |
| `encrypted_multivol_rar300.rar`, `.r00` | RAR 3 | `bigtext_64k.bin` | `password` |
| `multivol_newnaming_rar300.partNN.rar` | RAR 3 | `bigtext_64k.bin` | |
| `header_encrypted_newnaming_rar300.partNN.rar` | RAR 3, names encrypted | `bigtext_64k.bin` | `password` |
| `MULTIVOL.RAR`, `.R00`... | RAR 1.x | `RANDOM.BIN` | |
