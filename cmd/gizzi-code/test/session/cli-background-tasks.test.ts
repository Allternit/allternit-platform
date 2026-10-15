import { describe, expect, test } from "bun:test"
import fs from "node:fs/promises"
import path from "node:path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { SessionTasks } from "../../src/runtime/session/session-tasks"
import { CliBackgroundTasks } from "../../src/runtime/session/cli-background-tasks"
import { LocalCliDriver } from "../../src/runtime/drivers/local-cli-driver"
import { tmpdir } from "../fixture/fixture"

const call = (id: string) => ({
  type: "assistant",
  message: {
    content: [
      {
        type: "tool_use",
        id: `call_${id}`,
        name: "Bash",
        input: { run_in_background: true, description: `check ${id}` },
      },
    ],
  },
})
const result = (id: string, output: string) => ({
  type: "user",
  message: {
    content: [
      {
        type: "tool_result",
        tool_use_id: `call_${id}`,
        content: `Command running in background with ID: ${id}. Output is being written to: ${output}. You will be notified when it completes.`,
      },
    ],
  },
})

describe("CLI-owned background tasks", () => {
  test("mirrors a real tool result, polls bounded output, and keeps the first terminal outcome", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const dir = path.join(tmp.path, "tasks")
        await fs.mkdir(dir)
        const file = path.join(dir, "bg1.output")
        await fs.writeFile(file, "starting\n")
        const bridge = new CliBackgroundTasks(parent.id, async () => {})
        try {
          await bridge.observe(call("bg1"))
          await bridge.observe(result("bg1", file))
          const [row] = await SessionTasks.list(parent.id)
          expect(row).toMatchObject({
            kind: "shell",
            status: "running",
            output: "starting\n",
            description: "check bg1",
          })
          await bridge.observe({
            type: "system",
            subtype: "task_started",
            task_id: "bg1",
            task_type: "local_bash",
            description: "duplicate",
          })
          expect(await SessionTasks.list(parent.id)).toHaveLength(1)
          await fs.appendFile(file, "x".repeat(BackgroundTask.MAX_LIVE_OUTPUT + 10))
          await bridge.poll()
          expect((await BackgroundTask.get(row.id))?.output?.length).toBe(BackgroundTask.MAX_LIVE_OUTPUT)
          await bridge.observe({
            type: "system",
            subtype: "task_notification",
            task_id: "bg1",
            status: "completed",
            output_file: file,
            summary: "done",
          })
          await bridge.observe({
            type: "system",
            subtype: "task_notification",
            task_id: "bg1",
            status: "failed",
            summary: "late",
          })
          expect(await BackgroundTask.get(row.id)).toMatchObject({
            status: "completed",
          })
          expect(bridge.active).toBe(false)
        } finally {
          await bridge.close()
          await Session.remove(parent.id)
        }
      },
    })
  })

  test("scopes provider IDs to their parent and refuses arbitrary/symlinked output paths", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const a = await Session.create({})
        const b = await Session.create({})
        const one = new CliBackgroundTasks(a.id, async () => {})
        const two = new CliBackgroundTasks(b.id, async () => {})
        try {
          const secret = path.join(tmp.path, "private.txt")
          await fs.writeFile(secret, "not task output")
          const dir = path.join(tmp.path, "tasks")
          await fs.mkdir(dir)
          const file = path.join(dir, "bg1.output")
          await fs.symlink(secret, file)
          for (const bridge of [one, two]) {
            await bridge.observe(call("bg1"))
            await bridge.observe(result("bg1", file))
          }
          const [ra] = await SessionTasks.list(a.id)
          const [rb] = await SessionTasks.list(b.id)
          expect(ra.id).not.toBe(rb.id)
          expect(ra.output).toBeNull()
          expect(rb.output).toBeNull()
          await one.close("connection lost")
          expect(await BackgroundTask.get(ra.id)).toMatchObject({
            status: "interrupted",
            error: "connection lost",
          })
          expect((await BackgroundTask.get(rb.id))?.status).toBe("running")
        } finally {
          await one.close()
          await two.close()
          await Session.remove(a.id)
          await Session.remove(b.id)
        }
      },
    })
  })

  test("does not report cancellation when the provider rejects stop", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const bridge = new CliBackgroundTasks(parent.id, async () => {
          throw new Error("stop rejected")
        })
        try {
          await bridge.observe({
            type: "system",
            subtype: "task_started",
            task_id: "agent1",
            description: "research",
            task_type: "local_agent",
          })
          const [row] = await SessionTasks.list(parent.id)
          await expect(BackgroundTask.cancel(row.id)).rejects.toThrow("stop rejected")
          expect((await BackgroundTask.get(row.id))?.status).toBe("running")
          await bridge.observe({
            type: "system",
            subtype: "task_notification",
            task_id: "agent1",
            status: "failed",
            summary: "provider error",
          })
          expect(await BackgroundTask.get(row.id)).toMatchObject({
            kind: "subagent",
            status: "failed",
            error: "provider error",
          })
        } finally {
          await bridge.close()
          await Session.remove(parent.id)
        }
      },
    })
  })

  test("automatic Bash backgrounding and progress events do not require a requested background flag", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const bridge = new CliBackgroundTasks(parent.id, async () => {})
        try {
          const event = call("auto1")
          event.message.content[0].input.run_in_background = false
          await bridge.observe(event)
          await bridge.observe({
            ...result("auto1", "/missing/tasks/auto1.output"),
            tool_use_result: { backgroundTaskId: "auto1" },
          })
          const [row] = await SessionTasks.list(parent.id)
          expect(row).toMatchObject({ kind: "shell", status: "running" })
          await bridge.observe({
            type: "system",
            subtype: "task_started",
            task_id: "agent2",
            task_type: "local_agent",
            description: "review",
          })
          await bridge.observe({
            type: "system",
            subtype: "task_progress",
            task_id: "agent2",
            summary: "reviewing files",
          })
          const agent = (await SessionTasks.list(parent.id)).find((r) => r.kind === "subagent")!
          expect(agent.output).toBe("reviewing files")
          await bridge.observe({
            type: "system",
            subtype: "task_notification",
            task_id: "agent2",
            status: "unknown",
            summary: "do not claim success",
          })
          expect((await BackgroundTask.get(agent.id))?.status).toBe("running")
          await bridge.observe({ type: "assistant", message: { content: {} } })
        } finally {
          await bridge.close()
          await Session.remove(parent.id)
        }
      },
    })
  })

  test("Agent/Task async results survive a missing task-started event", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const bridge = new CliBackgroundTasks(parent.id, async () => {})
        try {
          const dir = path.join(tmp.path, "tasks")
          await fs.mkdir(dir)
          for (const [i, name] of ["Agent", "Task"].entries()) {
            const id = `agent${i}`
            const file = path.join(dir, `${id}.output`)
            await fs.writeFile(file, `progress ${id}`)
            await bridge.observe({
              type: "assistant",
              message: { content: [{ type: "tool_use", id, name, input: { description: "review" } }] },
            })
            await bridge.observe({
              type: "user",
              message: {
                content: [
                  {
                    type: "tool_result",
                    tool_use_id: id,
                    content: [
                      {
                        type: "text",
                        text: `Async agent launched successfully.\nagentId: ${id} (internal ID)\noutput_file: ${file}`,
                      },
                    ],
                  },
                ],
              },
            })
          }
          const tasks = await SessionTasks.list(parent.id)
          expect(tasks).toHaveLength(2)
          expect(
            tasks.every(
              (task) => task.kind === "subagent" && task.status === "running" && task.output?.startsWith("progress"),
            ),
          ).toBe(true)
        } finally {
          await bridge.close()
          await Session.remove(parent.id)
        }
      },
    })
  })

  test("a CLI crash leaves an interrupted task with its latest output", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const script = path.join(tmp.path, "crash.ts")
        await fs.writeFile(
          script,
          `
        console.log(JSON.stringify({type:'system',subtype:'task_started',task_id:'crash1',task_type:'local_bash',description:'crashing job'}));
        console.log(JSON.stringify({type:'result'}));
        setTimeout(() => process.exit(1),100);
      `,
        )
        const driver = new LocalCliDriver("fixture", "claude-cli")
        for await (const _ of (driver as any).runStreamJson(
          { taskId: "crash", runtimeId: "fixture", cliName: "claude-cli" },
          [process.execPath, script],
          {
            prompt: "check",
            promptOnStdin: false,
            endsWithResult: true,
            cwd: tmp.path,
            sessionID: parent.id,
          },
        )) {
        }
        const [row] = await SessionTasks.list(parent.id)
        expect(await BackgroundTask.wait(row.id, 3000)).toMatchObject({
          status: "interrupted",
        })
        await Bun.sleep(25)
        await Session.remove(parent.id)
      },
    })
  })

  test("the last task may stop before its control acknowledgement and persists before shutdown", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const script = path.join(tmp.path, "stop.ts")
        const persisted = path.join(tmp.path, "persisted")
        await fs.writeFile(
          script,
          `
          import fs from 'node:fs/promises';
          const emit = e => console.log(JSON.stringify(e));
          let started = false, buffer = '';
          process.stdin.on('data', chunk => {
            buffer += chunk.toString(); let i;
            while ((i = buffer.indexOf('\\n')) >= 0) {
              const line = buffer.slice(0,i); buffer = buffer.slice(i+1);
              if (!line.trim()) continue;
              const e = JSON.parse(line);
              if (e.type === 'control_request') {
                emit({type:'system',subtype:'task_notification',task_id:'only',status:'stopped'});
                setTimeout(() => emit({type:'control_response',response:{subtype:'success',request_id:e.request_id}}),100);
              } else if (!started) {
                started = true;
                emit({type:'system',subtype:'task_started',task_id:'only',task_type:'local_bash',description:'last job'});
                emit({type:'result'});
              }
            }
          });
          process.stdin.on('end', async () => {
            await new Promise(r => setTimeout(r,100));
            await fs.writeFile('persisted','terminal state saved');
          });
        `,
        )
        const driver = new LocalCliDriver("fixture", "claude-cli")
        for await (const _ of (driver as any).runStreamJson(
          { taskId: "stop-last", runtimeId: "fixture", cliName: "claude-cli" },
          [process.execPath, script],
          { prompt: "check", promptOnStdin: false, endsWithResult: true, cwd: tmp.path, sessionID: parent.id },
        )) {
        }
        const [row] = await SessionTasks.list(parent.id)
        await BackgroundTask.cancel(row.id)
        expect((await BackgroundTask.get(row.id))?.status).toBe("cancelled")
        for (let i = 0; i < 50; i++) {
          if (await fs.readFile(persisted, "utf8").catch(() => "")) break
          await Bun.sleep(20)
        }
        expect(await fs.readFile(persisted, "utf8")).toBe("terminal state saved")
        await Session.remove(parent.id)
      },
    })
  })

  test("foreground reply releases promptly; the retained CLI streams completion and targeted stop", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const script = path.join(tmp.path, "vendor.ts")
        await fs.writeFile(
          script,
          `
        import fs from 'node:fs/promises';
        const emit = (e) => console.log(JSON.stringify(e));
        const timers = new Map(); let started = false; let buf = '';
        process.stdin.on('data', async chunk => {
          buf += chunk.toString(); let i;
          while ((i = buf.indexOf('\\n')) >= 0) {
            const line = buf.slice(0,i); buf = buf.slice(i+1); if (!line.trim()) continue;
            const e = JSON.parse(line);
            if (e.type === 'control_response' && e.response?.request_id === 'background-permission') {
              await fs.appendFile(process.cwd()+'/tasks/bgB.output','permission '+e.response.response.behavior+'\\n');
              continue;
            }
            if (e.type === 'control_request') {
              clearTimeout(timers.get(e.request.task_id));
              emit({type:'control_response',response:{subtype:'success',request_id:e.request_id}});
              emit({type:'system',subtype:'task_notification',task_id:e.request.task_id,status:'stopped',summary:'stopped'});
              continue;
            }
            if (started) continue; started = true;
            await fs.mkdir('tasks',{recursive:true});
            for (const id of ['bgA','bgB']) {
              const file = process.cwd()+'/tasks/'+id+'.output'; await fs.writeFile(file,'first '+id+'\\n');
              emit({type:'assistant',message:{content:[{type:'tool_use',id:'call_'+id,name:'Bash',input:{run_in_background:true,description:'check '+id}}]}});
              emit({type:'user',message:{content:[{type:'tool_result',tool_use_id:'call_'+id,content:'Command running in background with ID: '+id+'. Output is being written to: '+file+'. You will be notified when it completes.'}]}});
              if (id === 'bgB') setTimeout(() => fs.appendFile(file,'middle '+id+'\\n'),300);
              timers.set(id,setTimeout(async () => {await fs.appendFile(file,'last '+id+'\\n');emit({type:'system',subtype:'task_notification',task_id:id,status:'completed',output_file:file,summary:'done'});},1500));
            }
            emit({type:'result',usage:{input_tokens:1,output_tokens:1}});
            setTimeout(() => emit({type:'control_request',request_id:'background-permission',request:{subtype:'can_use_tool',tool_name:'Write',input:{file_path:'no.txt'}}}),50);
          }
        });
      `,
        )
        const driver = new LocalCliDriver("fixture", "claude-cli")
        const start = Date.now()
        const events = []
        // Exercise the actual stream-json runner against a protocol fixture,
        // without launching a paid CLI or mocking lifecycle/storage boundaries.
        for await (const event of (driver as any).runStreamJson(
          { taskId: "fixture", runtimeId: "fixture", cliName: "claude-cli" },
          [process.execPath, script],
          {
            prompt: "check",
            promptOnStdin: false,
            endsWithResult: true,
            cwd: tmp.path,
            sessionID: parent.id,
            bot: { id: "restricted", allowedTools: ["file_read"] },
          },
        ))
          events.push(event)
        expect(events.some((e) => e.type === "finish")).toBe(true)
        expect(Date.now() - start).toBeLessThan(1400)
        const rows = await SessionTasks.list(parent.id)
        expect(rows).toHaveLength(2)
        const a = rows.find((r) => r.description === "check bgA")!
        const b = rows.find((r) => r.description === "check bgB")!
        expect(a.output).toBe("first bgA\n")
        await BackgroundTask.cancel(a.id)
        expect((await BackgroundTask.get(a.id))?.status).toBe("cancelled")
        expect((await BackgroundTask.get(b.id))?.status).toBe("running")
        await Bun.sleep(800)
        expect((await BackgroundTask.get(b.id))?.output).toBe("first bgB\npermission deny\nmiddle bgB\n")
        expect(await BackgroundTask.wait(b.id, 5000)).toMatchObject({
          status: "completed",
          output: "first bgB\npermission deny\nmiddle bgB\nlast bgB\n",
        })
        await Bun.sleep(50)
        await Session.remove(parent.id)
      },
    })
  })
})
