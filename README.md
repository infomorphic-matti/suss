# suss
Suss is a library to create collections of unix services that can interact with each other and start each other on-demand. 

Furthermore, it is pluggable into different async frameworks as long as you provide an implementation of [`socket_shims::UnixSocketInterface`] . If you don't care about async at all, either use the standard threadpool implementation (based on [`blocking`] ), or write a synchronous API implementation and use a crate like `pollster` to un-async things.

It is hosted at:
* <https://github.com/infomorphic-matti/suss>
* <https://gitlab.com/infomorphic-matti/suss>

## Basic Example
Provide a single interface for a simple echo service, over any compatible unix socket interface - with the ability to start on demand.

```rust,no_run
use suss::prelude::*;
use suss::blocking;
use std::io::{Result as IoResult, BufRead};

struct EchoClient<U: UnixSocketInterface>(U::UnixStream);

impl <U: UnixSocketInterface>  EchoClient<U> {
    pub fn new(u: U::UnixStream) -> Self {
        Self(u)
    }

    pub async fn write_and_receive<'b>(&mut self, in_value: &'b [u8]) -> IoResult<Vec<u8>> {
        U::unix_stream_write_all(&mut self.0, &(in_value.len() as u64).to_be_bytes()).await?;
        U::unix_stream_write_all(&mut self.0, in_value).await?;
        let mut out_length_bytes = [0u8;8];
        U::unix_stream_read_exact(&mut self.0, &mut out_length_bytes).await?;
        let mut out_length = u64::from_be_bytes(out_length_bytes);
        let mut out = vec![0u8;out_length.try_into().unwrap()];
        U::unix_stream_read_exact(&mut self.0, &mut out).await?;
        Ok(out)
    }
}

declare_service!{
    /// This is a very nice echo service!
    EchoService <U> = {
        "my-echo-service" "--server" @ "echo-service.socks" as raw |stream| -> Io<EchoClient<U>> { Ok(EchoClient(stream)) } 
    } impl {U: UnixSocketInterface}
}

fn main() { 
    let reified: ReifiedService<_, suss::socket_shims::StdThreadpoolUSocks, _> = EchoService.reify("/var/run/".as_ref());
    
    futures_lite_block_on(async move {
        let mut service_api = reified.connect_to_running().await.expect("No service running");
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let mut buf = String::new();
        loop {
            stdin.read_line(&mut buf).unwrap();
            if buf.trim() == "die" {
                break
            };
            let received_line = service_api.write_and_receive(buf.as_bytes()).await.unwrap();
            let valid_utf8 = std::str::from_utf8(&received_line).unwrap();
            println!("{valid_utf8}");
        }
    });
}

```

## Uses

This library allows you to:
* Define a list of services - and unix socket names - in a single location, along with an arbitrary interface wrapping a bare unix stream to act as a client connection, as well as a way to begin the service.
* Provide implementations of the service in another package, or perhaps the same package in a different module. In theory, this will allow alternate implementations or perhaps test implementations to flourish, as long as the service can keep the clients happy.
* Automatically attempt to detect running services on initial connection, and then try to start them in case a service was not running, along with an inbuilt mechanism for liveness checking (which is, admittedly, slightly intrusive in that it requires an argument to be passed to a new service in some manner).
* Uses the asynchronous unix sockets available in your async runtime if present, via optional dependencies (`async-std`, `tokio`, and a threadpool-based `std` fallback are currently available). All of these frameworks can convert to and from standard library unix sockets, so you can use them freely in client and server implementations even as `suss` remains runtime-agnostic.
* Provide a runtime directory as a "namespace" for your collection of services - this means that a collection of services can be run systemwide, per user, in some other configuration, all as long as you can provide a different base directory path for each collection of services you want to run.
