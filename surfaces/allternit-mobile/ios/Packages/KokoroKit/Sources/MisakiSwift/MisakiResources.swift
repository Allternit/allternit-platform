import Foundation

/// Allternit patch: the lexicons and BART fallback weights are downloaded on
/// first use instead of shipping in the app bundle. The app sets `directory`
/// to the folder holding `us_gold.json`, `us_bart.safetensors`, … before it
/// creates an `EnglishG2P`.
public enum MisakiResources {
  nonisolated(unsafe) public static var directory: URL?

  static func url(_ name: String, _ ext: String) -> URL? {
    guard let url = directory?.appendingPathComponent("\(name).\(ext)"),
          FileManager.default.fileExists(atPath: url.path) else { return nil }
    return url
  }
}
