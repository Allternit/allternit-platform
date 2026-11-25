// @ts-nocheck
import { BlockTitle } from '../../components/BlockTitle'
import * as path from 'node:path'
import * as React from 'react'
import {
  describeEntryMeta,
  estimateRenderedLines,
  formatDate,
  listArtifacts,
  readArtifactMarkdown,
  resolveArtifactsRoot,
  type ArtifactListEntry,
} from '@/runtime/artifacts/browse'
import { getOriginalCwd } from '../../bootstrap/state.js'
import { Select } from '../../components/CustomSelect/index.js'
import { Pane } from '../../components/design-system/Pane.js'
import { FilePathLink } from '../../components/FilePathLink.js'
import { Markdown } from '../../components/Markdown.js'
import { useTerminalSize } from '../../hooks/useTerminalSize.js'
import { Box, Text } from '../../ink.js'
import {
  useKeybinding,
  useKeybindings,
} from '../../keybindings/useKeybinding.js'
import type { LocalJSXCommandOnDone } from '../../types/command.js'
import { openFileInExternalEditor } from '../../utils/editor.js'
import { EmptyLine } from '../../components/EmptyLine'

interface ArtifactViewerProps {
  onDone: LocalJSXCommandOnDone
}

const VIEW_HEIGHT = 15

export function ArtifactViewer({ onDone }: ArtifactViewerProps) {
  const cwd = getOriginalCwd()
  const { root, source } = resolveArtifactsRoot(cwd)
  const displayRoot = path.join(source === 'gizzi' ? '.gizzi' : '.claude', 'artifacts')

  const [entries, setEntries] = React.useState<ArtifactListEntry[]>([])
  const [selected, setSelected] = React.useState<ArtifactListEntry | null>(null)
  const [content, setContent] = React.useState<string | null>(null)
  const [scrollOffset, setScrollOffset] = React.useState(0)
  const [status, setStatus] = React.useState<string | null>(null)
  const { columns } = useTerminalSize()

  React.useEffect(() => {
    setEntries(listArtifacts(root))
  }, [root])

  const estLines = content ? estimateRenderedLines(content, columns - 8) : 0
  const maxOffset = Math.max(0, estLines - VIEW_HEIGHT)

  // Viewer keys — only active while a file is open, so the Select in the
  // list view keeps its own up/down/enter/escape handling.
  useKeybindings(
    {
      'select:previous': () => setScrollOffset(prev => Math.max(0, prev - 1)),
      'select:next': () => setScrollOffset(prev => Math.min(maxOffset, prev + 1)),
      'artifacts:openInEditor': () => {
        if (!selected) return
        const target = selected.kind === 'canvas' ? selected.configPath : selected.filePath
        setStatus(
          openFileInExternalEditor(target)
            ? `opening ${target}`
            : `no $VISUAL/$EDITOR set — click the path above instead`,
        )
      },
      'artifacts:back': () => {
        setSelected(null)
        setContent(null)
        setScrollOffset(0)
        setStatus(null)
      },
    },
    { context: 'Artifacts', isActive: !!selected },
  )

  // Esc closes the command when there is nothing to select.
  useKeybinding('artifacts:back', onDone, {
    context: 'Artifacts',
    isActive: !selected && entries.length === 0,
  })

  const handleSelect = (value: string) => {
    const entry = entries.find(e =>
      e.kind === 'canvas' ? e.slug === value : e.name === value,
    )
    if (!entry) return
    try {
      setContent(readArtifactMarkdown(entry))
      setSelected(entry)
      setScrollOffset(0)
      setStatus(null)
    } catch {
      setStatus(`could not read ${value}`)
    }
  }

  if (entries.length === 0) {
    return (
      <Pane>
        <Box flexDirection="column" gap={1}>
          <EmptyLine>No artifacts yet.</EmptyLine>
          <Text dimColor>
            {`Artifacts live in ${displayRoot}/ — publish one with \`gizzi html-artifact publish --input <file.json>\`, or drop a markdown file in that directory.`}
          </Text>
          <Text dimColor>(press esc to close)</Text>
        </Box>
      </Pane>
    )
  }

  if (selected && content != null) {
    const title = selected.kind === 'canvas' ? selected.title : selected.name
    const targetPath = selected.kind === 'canvas' ? selected.configPath : selected.filePath
    return (
      <Pane>
        <Box flexDirection="column" marginBottom={1}>
          <Text bold color="cyan">
            Viewing: {title}
          </Text>
          <Text dimColor>
            <FilePathLink filePath={targetPath}>{targetPath}</FilePathLink>
          </Text>
          <Text dimColor>(↑/↓ scroll · e open in editor · esc back)</Text>
        </Box>
        <Box
          flexDirection="column"
          borderStyle="single"
          borderColor="gray"
          paddingX={1}
          height={VIEW_HEIGHT}
          overflowY="hidden"
        >
          <Box flexDirection="column" marginTop={-scrollOffset} flexShrink={0}>
            <Markdown>{content}</Markdown>
          </Box>
        </Box>
        <Box marginTop={1} flexDirection="column">
          <Text dimColor>
            line {Math.min(scrollOffset + 1, estLines)}–
            {Math.min(scrollOffset + VIEW_HEIGHT, estLines)} of ~{estLines}
          </Text>
          {status && <Text dimColor>{status}</Text>}
        </Box>
      </Pane>
    )
  }

  const options = entries.map(e => ({
    label: e.kind === 'canvas' ? `${e.title}` : e.name,
    value: e.kind === 'canvas' ? e.slug : e.name,
    description:
      e.kind === 'canvas'
        ? `${e.slug} · ${describeEntryMeta(e)}`
        : describeEntryMeta(e),
  }))

  return (
    <Pane>
      <Box marginBottom={1} flexDirection="column">
        <BlockTitle title="ARTIFACTS" />
        <Text dimColor>
          {entries.length} artifacts · {displayRoot}/
        </Text>
        <Text dimColor>updated {formatDate(entries[0]?.mtimeMs ?? 0)}</Text>
      </Box>
      <Select
        options={options}
        onChange={handleSelect}
        onCancel={onDone}
        visibleOptionCount={8}
      />
      <Box marginTop={1} flexDirection="column">
        <Text dimColor>(enter to view · esc to close)</Text>
        {status && <Text dimColor>{status}</Text>}
      </Box>
    </Pane>
  )
}

export async function call(
  onDone: LocalJSXCommandOnDone,
): Promise<React.ReactNode> {
  return <ArtifactViewer onDone={onDone} />
}
