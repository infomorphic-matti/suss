# suss
Suss is a library to create collections of unix services that can interact with each other and start each other on-demand.

This library allows you to:
* Define a list of services - and unix socket names - in a single location, along with an arbitrary interface wrapping a bare unix stream to act as a client connection, as well as a way to begin the service.
* Provide implementations of the service in another package, or perhaps the same package in a different module. In theory, this will allow alternate implementations or perhaps test implementations to flourish, as long as the service can keep the clients happy.
* Automatically attempt to detect running services on initial connection, and then try to start them in case a service was not running, along with an inbuilt mechanism for liveness checking (which is, admittedly, slightly intrusive in that it requires an argument to be passed to a new service in some manner).
* Uses the asynchronous unix sockets available in your async runtime if present, via optional dependencies (`async-std`, `tokio`, and a threadpool-based `std` fallback are currently available). All of these frameworks can convert to and from standard library unix sockets, so you can use them freely in client and server implementations even as `suss` remains runtime-agnostic.
* Provide a runtime directory as a "namespace" for your collection of services - this means that a collection of services can be run systemwide, per user, in some other configuration, all as long as you can provide a different base directory path for each collection of services you want to run.


Note that this library is still in very early stages, and none of the convenience functions exist yet, nor unit tests, however it should be usable.
