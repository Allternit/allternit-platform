import SwiftUI

/// Sign-in for a computer's own screen sharing, shown over the screen viewer
/// while `BotDesktopSession.state` is `.needsCredentials` (the web's
/// ComputerViewport asks the same way). A Mac asks for the user name and
/// password of an account on that Mac; other VNC servers ask for a password.
struct RFBCredentialsForm: View {
    let needsUsername: Bool
    let message: String?
    let onSubmit: (_ username: String, _ password: String) -> Void

    @State private var username = ""
    @State private var password = ""
    @FocusState private var focused: Field?

    private enum Field { case username, password }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Sign in to this computer")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
            Text(needsUsername
                 ? "Use the account name and password you log in to this computer with (not your Allternit sign-in)."
                 : "Its screen sharing asks for a password.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
            if let message {
                Label(message, systemImage: "exclamationmark.circle")
                    .font(.footnote)
                    .foregroundColor(.red)
            }
            if needsUsername {
                TextField("Account name", text: $username)
                    .textContentType(.username)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.next)
                    .focused($focused, equals: .username)
                    .onSubmit { focused = .password }
                    .textFieldStyle(.roundedBorder)
            }
            SecureField("Password", text: $password)
                .textContentType(.password)
                .submitLabel(.go)
                .focused($focused, equals: .password)
                .onSubmit(submit)
                .textFieldStyle(.roundedBorder)
            Button(action: submit) {
                Text("Connect")
                    .font(.headline)
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .tint(Color("AccentPrimary"))
            .disabled(!canSubmit)
        }
        .padding(20)
        .frame(maxWidth: 360)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .padding(20)
        .onAppear { focused = needsUsername ? .username : .password }
    }

    private var canSubmit: Bool {
        !password.isEmpty && (!needsUsername || !username.trimmingCharacters(in: .whitespaces).isEmpty)
    }

    private func submit() {
        guard canSubmit else { return }
        onSubmit(username.trimmingCharacters(in: .whitespaces), password)
        password = ""
    }
}
