use std::env;
use std::fs;
use std::path::PathBuf;
use wit_component::ComponentEncoder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let input = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: cargo run --example componentize -- <core.wasm> <component.wasm>")?;
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: cargo run --example componentize -- <core.wasm> <component.wasm>")?;
    if args.next().is_some() {
        return Err("componentize accepts exactly two paths".into());
    }

    let module = fs::read(&input)?;
    let component = ComponentEncoder::default()
        .module(&module)?
        .validate(true)
        .encode()?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, component)?;
    Ok(())
}
