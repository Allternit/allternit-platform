import { Hono } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { Config } from "@/runtime/context/config/config"
import { Provider } from "@/runtime/providers/provider"
import { mapValues } from "remeda"
import { errors } from "@/runtime/server/error"
import { Log } from "@/shared/util/log"
import { lazy } from "@/shared/util/lazy"

const log = Log.create({ service: "server" })

export const ConfigRoutes = lazy(() =>
  new Hono()
    .get(
      "/",
      describeRoute({
        summary: "Get configuration",
        description: "Retrieve the current GIZZI configuration settings and preferences.",
        operationId: "config.get",
        responses: {
          200: {
            description: "Get config info",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        return c.json(await Config.get())
      },
    )
    .patch(
      "/",
      describeRoute({
        summary: "Update configuration",
        description: "Update GIZZI configuration settings and preferences.",
        operationId: "config.update",
        responses: {
          200: {
            description: "Successfully updated config",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400),
        },
      }),
      validator("json", z.any()),
      async (c) => {
        const config = c.req.valid("json") as any
        await Config.update(config)
        return c.json(config)
      },
    )
    .patch(
      "/global",
      describeRoute({
        summary: "Update global configuration",
        description:
          "Deep-merge settings into the user's global GIZZI config (not a project's). Used by the platform API to apply account-level preferences such as the browser tool's default adapter and browser permission rules.",
        operationId: "config.updateGlobal",
        responses: {
          200: {
            description: "Successfully updated global config",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(400),
        },
      }),
      validator("json", z.any()),
      async (c) => {
        const config = c.req.valid("json") as any
        await Config.updateGlobal(config)
        return c.json(config)
      },
    )
    .get(
      "/providers",
      describeRoute({
        summary: "List config providers",
        description: "Get a list of all configured AI providers and their default models.",
        operationId: "config.providers",
        responses: {
          200: {
            description: "List of providers",
            content: {
              "application/json": {
                schema: resolver(
                  z.object({
                    providers: Provider.Info.array(),
                    default: z.record(z.string(), z.string()),
                  })
                ),
              },
            },
          },
        },
      }),
      async (c) => {
        using _ = log.time("providers")
        const providers = await Provider.list().then((x) => mapValues(x, (item) => item))
        return c.json({
          providers: Object.values(providers),
          default: mapValues(providers, (item) => Provider.sort(Object.values(item.models))[0].id),
        })
      },
    ),
)
