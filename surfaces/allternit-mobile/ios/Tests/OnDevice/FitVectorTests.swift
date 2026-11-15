import XCTest
@testable import Allternit

/// Runs every golden vector of the fit spec against the Swift port.
/// The vectors file is the canonical `spec/on-device/fit-vectors.json`
/// itself, copied into this bundle at build time by project.yml (so it is
/// byte-identical by construction and cannot drift).
final class FitVectorTests: XCTestCase {
    private struct Vector: Decodable {
        struct Expect: Decodable {
            let k: FitKind
            let label: String
            let quant: String?
        }
        let name: String
        let budget: DeviceBudget
        let quants: [Quant]?
        let expect: Expect
    }

    func testEveryFitVector() throws {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "fit-vectors", withExtension: "json"),
                                "fit-vectors.json missing from the test bundle")
        let vectors = try JSONDecoder().decode([Vector].self, from: Data(contentsOf: url))
        XCTAssertEqual(vectors.count, 59, "the canonical file has 59 vectors; update this if the spec adds more")
        for v in vectors {
            let r = Fit.fit(v.budget, v.quants)
            XCTAssertEqual(r.k, v.expect.k, v.name)
            XCTAssertEqual(r.label, v.expect.label, v.name)
            XCTAssertEqual(r.quant?.name, v.expect.quant, v.name)
        }
    }

    /// The `why` strings are not in the vectors; pin the GB formatting rules
    /// from fit.md (N always one decimal, B drops a trailing .0).
    func testWhyFormatting() {
        let phone = DeviceBudget(kind: .unified, fastBytes: 2_297_000_000, spillBytes: 0, apple: true)
        let q = Quant(name: "4bit", format: "mlx", engine: .mlx, size: 968_080_210)
        XCTAssertEqual(Fit.fit(phone, [q]).why, "1.6 of 2.3 GB for models")
        let big = Quant(name: "4bit", format: "mlx", engine: .mlx, size: 2_263_022_417)
        XCTAssertEqual(Fit.fit(phone, [big]).why, "Needs 3.1 GB, 2.3 GB free for models")
        let gpu = DeviceBudget(kind: .discrete, fastBytes: 12e9, spillBytes: 32e9)
        let g = Quant(name: "Q4_K_M", format: "gguf", engine: .llamaCpp, size: 2_700_000_000)
        XCTAssertEqual(Fit.fit(gpu, [g]).why, "Full GPU, 3.6 of 12 GB VRAM")
    }
}
