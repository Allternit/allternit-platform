import Foundation

extension FilesSharedStore {
    /// A relay-only `FilesClient` for `computerId`, configured from what the
    /// app shared: the cloud URL, this device's id and name, and the Files
    /// token from the shared keychain (read on every request, so a token the
    /// app replaces is picked up without restarting the extension).
    ///
    /// `transferHTTP` carries bulk bytes; the File Provider extension passes
    /// a `BackgroundFilesHTTP`. `allowedTransports` stays `[.relay]` in any
    /// extension (spike 5: no tsnet there, iOS kills it at 20,481 KB).
    public func makeClient(computerId: ComputerId,
                           transferHTTP: FilesHTTP? = nil,
                           allowedTransports: Set<TransportKind> = [.relay],
                           devicePlatform: String = "ios") -> FilesClient {
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
}
