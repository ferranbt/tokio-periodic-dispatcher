
# Tokio-periodic-dispatcher

A library to periodically dispatch actions in rust.

```
# Cargo.toml
[dependencies]
tokio = "0.1.3"
tokio-timer = "0.1"
tokio-periodic-dispatcher = {git="https://github.com/ferranbt/tokio-periodic-dispatcher"}
```

## Usage

```
extern crate tokio_timer;

use tokio_timer::{Timer};

#[derive(Debug, Clone)]
enum Message {
    Hello,
    World
}

fn main() {

    let mut periodic = PeriodicDispatcher::new(Timer::default());
    let handle = periodic.handle();

    let periodic = periodic.for_each(|msg| {
        println!("msg = {:?}", msg);

        Ok(())
    })
    .map_err(|e| panic!("err = {:?}", e));
    
    handle.register(Duration::from_secs(1), Message::Hello);
    handle.register(Duration::from_secs(2), Message::World);

    tokio.run(periodic);
}
```
