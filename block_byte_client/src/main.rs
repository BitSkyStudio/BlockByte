use std::path::Path;

use block_byte_common::registry::load_registries;

fn main() {
    load_registries(&Path::new("assets"));
}
