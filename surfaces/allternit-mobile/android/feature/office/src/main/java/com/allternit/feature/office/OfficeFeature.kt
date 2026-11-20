package com.allternit.feature.office

import androidx.compose.runtime.Composable
import com.allternit.feature.artifacts.ArtifactOfficeHooks
import com.allternit.feature.artifacts.V2ArtifactScreen
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.chat.ChatBackend

/** The entry points :feature:shell wires: the artifact hooks (Sign PDF, sectioned documents, the library's Allternit Office) and the bots floor. */
object OfficeFeature {
    /** The composer's picked model and effort, as the shell's ModelStore holds them (iOS ModelStore.selectedModelId / effortForSend). */
    @Volatile var selectedModel: () -> Pair<String?, String?> = { null to null }

    /** Opens a chat thread in the shell: "Go to chat" on a document opened from the Office list. */
    @Volatile var openChat: (sessionId: String) -> Unit = {}

    /** Sets the [ArtifactOfficeHooks] the artifacts screens open Office from. Safe to call again. */
    fun install() {
        ArtifactOfficeHooks.signPdf = { title, pdf, onClose -> PdfSignScreen(title = title, source = pdf, onClose = onClose) }
        ArtifactOfficeHooks.legacyDocument = { legacyId, onClose -> OfficeDocumentScreen(legacyId, onClose) }
        ArtifactOfficeHooks.documents = { onClose ->
            OfficeDocumentsScreen(onClose = onClose, openArtifact = { id, _, close -> V2ArtifactScreen(artifactId = id, onClose = close, onOpenChat = { sid -> close(); onClose(); openChat(sid) }, onSignIn = {}) })
        }
    }

    /** `BotsHost.officeFloor`: Bots options, Office. */
    val floor: @Composable (store: BotModeStore, backend: ChatBackend, openBot: (String) -> Unit, onBack: () -> Unit) -> Unit =
        { store, backend, openBot, _ -> OfficeFloorScreen(store, backend, openBot) }
}
