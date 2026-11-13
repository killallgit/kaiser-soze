# Parse, don't validate — Rust

Validating: every caller holds a `String` and must remember the check.

```rust
fn is_valid_port(raw: &str) -> bool {
    raw.parse::<u16>().is_ok_and(|p| p != 0)
}

fn connect(host: &str, port: &str) -> io::Result<TcpStream> {
    if !is_valid_port(port) {
        return Err(io::Error::other("bad port"));
    }
    TcpStream::connect((host, port.parse::<u16>().unwrap()))
}
```

Parsing: the boundary builds a `Port`; `connect` cannot receive a bad one.

```rust
#[derive(Clone)]
pub struct Port(NonZeroU16);

impl FromStr for Port {
    type Err = PortError;

    fn from_str(raw: &str) -> Result<Self, PortError> {
        let n: u16 = raw.parse().map_err(|_| PortError::NotANumber)?;
        NonZeroU16::new(n).map(Port).ok_or(PortError::Zero)
    }
}

fn connect(host: &str, port: Port) -> io::Result<TcpStream> {
    TcpStream::connect((host, port.0.get()))
}
```

With clap's derive API, the boundary is the argument definition: a field typed `Port` is parsed
through this `FromStr` impl, and a bad value fails before your code runs. Clap needs `Port: Clone` and
`PortError: std::error::Error`.
