import UIKit

/// Custom keyboard (public API). It types only a job that the app queued after
/// the runtime's phone guard answered `allow` (see GuardClient in the app). It
/// never decides on its own what to type. It needs "Allow Full Access" to read
/// the app group. It cannot type into secure fields or apps that refuse
/// third-party keyboards; the system switches to the stock keyboard there.
final class KeyboardViewController: UIInputViewController {
    private let status = UILabel()
    private let typeButton = UIButton(type: .system)
    private let nextButton = UIButton(type: .system)
    private var typing = false

    override func viewDidLoad() {
        super.viewDidLoad()
        status.font = .preferredFont(forTextStyle: .footnote)
        status.numberOfLines = 2
        status.text = "OnPhone Keys"
        typeButton.setTitle("Type queued job", for: .normal)
        typeButton.addTarget(self, action: #selector(typeQueued), for: .touchUpInside)
        nextButton.setTitle("Next keyboard", for: .normal)
        nextButton.addTarget(self, action: #selector(handleInputModeList(from:with:)), for: .allTouchEvents)
        let stack = UIStackView(arrangedSubviews: [status, typeButton, nextButton])
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
            stack.topAnchor.constraint(equalTo: view.topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -8),
            view.heightAnchor.constraint(equalToConstant: 150),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if !hasFullAccess {
            status.text = "Turn on Allow Full Access for OnPhone Keys."
            return
        }
        // Auto-type a fresh job so the measurement has no human delay in it.
        if let job = Spike.readJSON(Spike.typeJob), (job["state"] as? String) == "queued",
           let created = job["createdAt"] as? Double, Spike.now - created < 120 {
            typeQueued()
        }
    }

    @objc private func typeQueued() {
        guard !typing else { return }
        guard var job = Spike.readJSON(Spike.typeJob), (job["state"] as? String) == "queued",
              let text = job["text"] as? String, let id = job["id"] as? String else {
            status.text = "No queued job."
            return
        }
        // The job exists only after the guard allowed it; recheck the stamp anyway.
        guard (job["guard"] as? String) == "allow" else {
            status.text = "Job not allowed by the guard."
            return
        }
        typing = true
        job["state"] = "typing"
        Spike.writeJSON(Spike.typeJob, job)
        let proxy = textDocumentProxy
        let t0 = CFAbsoluteTimeGetCurrent()
        for ch in text { proxy.insertText(String(ch)) }
        let ms = (CFAbsoluteTimeGetCurrent() - t0) * 1000
        // Let the host apply the inserts before reading the context back.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
            guard let self else { return }
            let before = proxy.documentContextBeforeInput ?? ""
            let tail = String(text.suffix(min(text.count, before.count)))
            let tailMatches = !before.isEmpty && before.hasSuffix(tail)
            Spike.writeJSON(Spike.typeResult, [
                "id": id, "target": job["target"] ?? "", "chars": text.count, "insertMs": ms,
                "contextBefore": before, "contextChars": before.count, "tailMatches": tailMatches,
                "ts": Spike.now,
            ])
            job["state"] = "done"
            Spike.writeJSON(Spike.typeJob, job)
            self.status.text = "Typed \(text.count) chars in \(Int(ms)) ms."
            self.typing = false
        }
    }
}
