`nice-plug` 0.4.2 from crates.io with one fix in
`src/wrapper/vst3/wrapper.rs`: `setBusArrangements` used an inverted index for
the first auxiliary bus (`if has_main_input { 0 } else { 1 }` instead of
`{ 1 } else { 0 }`, and the same for outputs), so hosts could not configure a
plugin that has an auxiliary input but no main input. `getBusInfo` already
used the correct index.
