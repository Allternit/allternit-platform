// @ts-nocheck
import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import { BlockTitle } from '../../components/BlockTitle'
import { Dialog } from '../../components/design-system/Dialog'
import type { LocalJSXCommandCall } from '../../types/command'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'
import { EmptyLine } from '../../components/EmptyLine'

const API_BASE = ALLTERNIT_GATEWAY_BASE

interface CoworkProject {
  id: string
  title: string
  description?: string
  instructions?: string
  created_at: string
}

interface TaskItem {
  id: string
  title: string
  status: string
  assignee_type?: string
  assignee_name?: string
  created_at?: string
}

async function apiGet(path: string) {
  const res = await fetch(`${API_BASE}${path}`)
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return res.json()
}

async function apiPost(path: string, body: unknown) {
  const res = await fetch(`${API_BASE}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return res.json()
}

function formatStatus(status?: string): string {
  if (!status) return 'pending'
  return status
}

export function CoworkProjectCommandWrapper({
  onDone,
}: {
  onDone: (result?: string) => void
}) {
  const [projects, setProjects] = React.useState<CoworkProject[]>([])
  const [tasks, setTasks] = React.useState<TaskItem[]>([])
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState<string | null>(null)
  const [selectedIndex, setSelectedIndex] = React.useState(0)
  const [selectedProjectId, setSelectedProjectId] = React.useState<string | null>(null)
  const [newTaskTitle, setNewTaskTitle] = React.useState('')
  const [isCreating, setIsCreating] = React.useState(false)
  const [view, setView] = React.useState<'list' | 'detail' | 'input'>('list')

  const fetchProjects = React.useCallback(async () => {
    try {
      const data = await apiGet('/api/v1/cowork/projects')
      setProjects(data.projects || [])
      setLoading(false)
    } catch (e) {
      setError(String(e))
      setLoading(false)
    }
  }, [])

  const fetchTasks = React.useCallback(async (projectId: string) => {
    try {
      const data = await apiGet(`/api/v1/tasks?workspace_id=${encodeURIComponent(projectId)}`)
      setTasks(data.tasks || [])
    } catch (e) {
      setTasks([])
    }
  }, [])

  React.useEffect(() => {
    fetchProjects()
  }, [fetchProjects])

  React.useEffect(() => {
    if (selectedProjectId) {
      fetchTasks(selectedProjectId)
    }
  }, [selectedProjectId, fetchTasks])

  useInput((input, key) => {
    if (view === 'input') {
      if (key.return) {
        void handleCreateTask()
      } else if (key.escape) {
        setView('detail')
        setNewTaskTitle('')
      } else if (key.backspace || key.delete) {
        setNewTaskTitle((t) => t.slice(0, -1))
      } else if (input && !key.ctrl && !key.meta && input.length === 1) {
        setNewTaskTitle((t) => t + input)
      }
      return
    }

    if (view === 'list') {
      if (key.upArrow) {
        setSelectedIndex((i) => Math.max(0, i - 1))
      } else if (key.downArrow) {
        setSelectedIndex((i) => Math.min(projects.length - 1, i + 1))
      } else if (key.return && projects[selectedIndex]) {
        setSelectedProjectId(projects[selectedIndex].id)
        setView('detail')
      }
    } else if (view === 'detail') {
      if (key.escape) {
        setView('list')
        setSelectedProjectId(null)
        setNewTaskTitle('')
      } else if (input === 'n') {
        setView('input')
      }
    }

    if (input === 'q') {
      onDone()
    }
  })

  const handleCreateTask = async () => {
    if (!selectedProjectId || !newTaskTitle.trim() || isCreating) return
    setIsCreating(true)
    try {
      await apiPost('/api/v1/tasks', {
        title: newTaskTitle.trim(),
        workspace_id: selectedProjectId,
        status: 'todo',
      })
      setNewTaskTitle('')
      await fetchTasks(selectedProjectId)
    } catch (e) {
      setError(String(e))
    } finally {
      setIsCreating(false)
    }
  }

  const selectedProject = projects.find((p) => p.id === selectedProjectId)

  if (loading) {
    return (
      <Dialog title={<BlockTitle title="PROJECTS" />} onCancel={() => onDone()}>
        <Text>Loading projects…</Text>
      </Dialog>
    )
  }

  if (error && !selectedProjectId) {
    return (
      <Dialog title={<BlockTitle title="PROJECTS" />} onCancel={() => onDone()}>
        <Text color="red">Error: {error}</Text>
      </Dialog>
    )
  }

  if (view === 'detail' && selectedProject) {
    const humanTasks = tasks.filter((t) => t.assignee_type !== 'agent')
    const agentTasks = tasks.filter((t) => t.assignee_type === 'agent')

    return (
      <Dialog
        title={`Project: ${selectedProject.title}`}
        subtitle={selectedProject.description || undefined}
        onCancel={() => {
          setView('list')
          setSelectedProjectId(null)
          setNewTaskTitle('')
        }}
      >
        <Box flexDirection="column">
          {selectedProject.instructions ? (
            <Box marginBottom={1}>
              <Text dimColor>Instructions: {selectedProject.instructions}</Text>
            </Box>
          ) : null}

          <Box marginBottom={1}>
            <Text bold>Tasks</Text>
          </Box>
          {humanTasks.length === 0 ? (
            <Text dimColor>No tasks yet.</Text>
          ) : (
            humanTasks.map((task) => (
              <Box key={task.id}>
                <Text>
                  • {task.title} <Text dimColor>({formatStatus(task.status)})</Text>
                </Text>
              </Box>
            ))
          )}

          <Box marginTop={1} marginBottom={1}>
            <Text bold>Agent Tasks</Text>
          </Box>
          {agentTasks.length === 0 ? (
            <Text dimColor>No agent tasks yet.</Text>
          ) : (
            agentTasks.map((task) => (
              <Box key={task.id}>
                <Text>
                  • {task.title} <Text dimColor>({formatStatus(task.status)})</Text>
                </Text>
              </Box>
            ))
          )}

          {view === 'input' ? (
            <Box marginTop={1}>
              <Text color="green">New task: {newTaskTitle}</Text>
              {isCreating ? <Text dimColor> creating…</Text> : null}
              <Text dimColor>  Enter save · Esc cancel</Text>
            </Box>
          ) : (
            <Box marginTop={1}>
              <Text dimColor>n new task · Esc back · q quit</Text>
            </Box>
          )}
        </Box>
      </Dialog>
    )
  }

  return (
    <Dialog title={<BlockTitle title="PROJECTS" />} onCancel={() => onDone()}>
      <Box flexDirection="column">
        {projects.length === 0 ? (
          <EmptyLine>No Cowork projects found.</EmptyLine>
        ) : (
          projects.map((project, index) => (
            <Box key={project.id}>
              <Text color={index === selectedIndex ? 'green' : undefined}>
                {index === selectedIndex ? '› ' : '  '}
                {project.title}
              </Text>
            </Box>
          ))
        )}
        <Box marginTop={1}>
          <Text dimColor>↑/↓ select · Enter open · q quit</Text>
        </Box>
      </Box>
    </Dialog>
  )
}

export const call: LocalJSXCommandCall = async (onDone) => {
  return <CoworkProjectCommandWrapper onDone={onDone} />
}
