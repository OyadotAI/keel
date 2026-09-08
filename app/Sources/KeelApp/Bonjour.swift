import Foundation

/// Telling the network this Keel exists, but only when it is actually reachable.
///
/// Advertised from Swift rather than Rust on purpose: this is a dozen lines here, and adding an
/// mDNS crate to the daemon would be a second implementation of service discovery to keep working
/// for the sake of moving code across a boundary.
///
/// `NetService` rather than `NWListener`, which is the modern API and the wrong one: `NWListener`
/// advertises a port it has bound, and the port being advertised here belongs to the daemon in
/// another process. Binding it would fail with "address in use", and binding a different one would
/// publish an SRV record pointing at a port that serves nothing. `NetService` publishes a record
/// for a port it does not own, which is exactly the job.
///
/// Nothing is advertised while the daemon is on loopback. A record pointing at an address nobody
/// off this machine can reach is an invitation to a connection that will always fail.
@MainActor
final class Bonjour {
    private var service: NetService?

    func advertise(port: UInt16, repo: String) {
        stop()
        let s = NetService(domain: "local.", type: "_keel._tcp.",
                           name: "Keel — \(repo)", port: Int32(port))
        var txt: [String: Data] = [:]
        txt["port"] = Data(String(port).utf8)
        txt["repo"] = Data(repo.utf8)
        s.setTXTRecord(NetService.data(fromTXTRecord: txt))
        s.publish()
        service = s
    }

    func stop() {
        service?.stop()
        service = nil
    }
}
