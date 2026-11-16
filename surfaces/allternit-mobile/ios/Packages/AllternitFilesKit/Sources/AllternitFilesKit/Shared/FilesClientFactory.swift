import Foundation

extension FilesSharedStore {
    /// A relay-only `FilesClient` for `computerId`, configured from what the
    /// app shared: the cloud URL, this device's id and name, and the Files
    /// token from the shared keychain (read on every request, so a token the
    /// app replaces is picked up without restarting the extension).
    ///
    /// `transferHTTP` carries bulk bytes; the File Provider extension passes
    /// a `BackgroundFilesHTTP`. `allowedTransports` defaults to
    /// `extensionTransports`.
    public func makeClient(computerId: ComputerId,
                           transferHTTP: FilesHTTP? = nil,
                           allowedTransports: Set<TransportKind> = FilesSharedStore.extensionTransports,
                           devicePlatform: String = FilesSharedStore.devicePlatform) -> FilesClient {
        let configuration = FilesClientConfiguration(
            cloudBaseURL: cloudBaseURL,
            tokenProvider: { [self] in self.tokenRejected ? nil : self.token() },
            deviceId: deviceId ?? "",
            deviceName: deviceName,
            devicePlatform: devicePlatform,
            allowedTransports: allowedTransports,
            transferHTTP: transferHTTP,
            scratchDirectory: scratchDirectory(for: computerId),
            sharedStore: self
        )
        return FilesClient(computerId: computerId, configuration: configuration)
    }

    /// What a File Provider extension may use. iOS: relay only (spike 5: no
    /// tsnet in the extension, iOS kills it at 20,481 KB). macOS: mesh too,
    /// through the Desktop app's loopback mesh proxy (`meshProxies`), for
    /// computers that are only on the mesh (SPEC section 10), then relay.
    public static var extensionTransports: Set<TransportKind> {
        #if os(macOS)
        return [.mesh, .relay]
        #else
        return [.relay]
        #endif
    }

    /// `X-Allternit-Device-Platform` of this device.
    public static var devicePlatform: String {
        #if os(macOS)
        return "macos"
        #else
        return "ios"
        #endif
    }
}
