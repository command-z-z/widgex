fn main() -> anyhow::Result<()> {
    widgex::run(std::env::args_os())?.print();
    Ok(())
}
