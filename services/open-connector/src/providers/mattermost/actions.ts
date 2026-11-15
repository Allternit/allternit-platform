import type { ProviderActionDefinition } from "../../core/provider-definition.ts";

import { s } from "../../core/json-schema.ts";
import { defineProviderAction } from "../../core/provider-definition.ts";

const service = "mattermost";

const emptyInputSchema = s.object("No input parameters are required.", {});
const rawResponseSchema = s.unknown("The raw Mattermost API JSON response.");
const entitySchema = s.looseObject("A Mattermost API entity object.");
const postSchema = s.looseObject("A Mattermost post object.");
const idSchema = s.string("A Mattermost object ID.", { minLength: 1 });
const pageSchema = s.nonNegativeInteger("The zero-based page number to request.");
const perPageSchema = s.integer("The number of records to request per page.", { minimum: 1 });

export const mattermostActions: ProviderActionDefinition[] = [
  defineProviderAction(service, {
    name: "get_current_user",
    description: "Get the Mattermost user associated with the Personal Access Token.",
    inputSchema: emptyInputSchema,
    outputSchema: s.object("Output payload for the current Mattermost user.", {
      user: entitySchema,
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "list_users",
    description: "List Mattermost users (names and usernames), optionally only those in one team or channel.",
    inputSchema: s.object(
      "Query parameters for listing Mattermost users.",
      {
        inTeam: s.string("Only users in this team ID.", { minLength: 1 }),
        inChannel: s.string("Only users in this channel ID.", { minLength: 1 }),
        page: pageSchema,
        perPage: s.integer("The number of users per page (Mattermost allows up to 200).", { minimum: 1, maximum: 200 }),
      },
      { optional: ["inTeam", "inChannel", "page", "perPage"] },
    ),
    outputSchema: s.object("Output payload for Mattermost users.", {
      users: s.array("Mattermost users returned by the API.", entitySchema),
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "list_user_teams",
    description: "List Mattermost teams visible to the current user.",
    inputSchema: emptyInputSchema,
    outputSchema: s.object("Output payload for Mattermost teams.", {
      teams: s.array("Mattermost teams returned by the API.", entitySchema),
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "get_team",
    description: "Retrieve one Mattermost team by ID.",
    inputSchema: s.object("Input parameters for retrieving one Mattermost team.", {
      teamId: idSchema,
    }),
    outputSchema: s.object("Output payload for one Mattermost team.", {
      team: entitySchema,
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "list_team_channels",
    description: "List public Mattermost channels in a team.",
    inputSchema: s.object(
      "Query parameters for listing Mattermost channels in a team.",
      {
        teamId: idSchema,
        page: pageSchema,
        perPage: perPageSchema,
      },
      { optional: ["page", "perPage"] },
    ),
    outputSchema: s.object("Output payload for Mattermost channels.", {
      channels: s.array("Mattermost channels returned by the API.", entitySchema),
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "get_channel",
    description: "Retrieve one Mattermost channel by ID.",
    inputSchema: s.object("Input parameters for retrieving one Mattermost channel.", {
      channelId: idSchema,
    }),
    outputSchema: s.object("Output payload for one Mattermost channel.", {
      channel: entitySchema,
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "list_channel_posts",
    description: "List Mattermost posts in a channel.",
    inputSchema: s.object(
      "Query parameters for listing posts in a Mattermost channel. since cannot be used with page, perPage, beforePostId, or afterPostId.",
      {
        channelId: idSchema,
        page: pageSchema,
        perPage: perPageSchema,
        since: s.nonNegativeInteger("Only return posts created after this Unix timestamp in milliseconds."),
        beforePostId: s.string("Return posts before this Mattermost post ID.", { minLength: 1 }),
        afterPostId: s.string("Return posts after this Mattermost post ID.", { minLength: 1 }),
      },
      { optional: ["page", "perPage", "since", "beforePostId", "afterPostId"] },
    ),
    outputSchema: s.object("Output payload for Mattermost channel posts.", {
      posts: s.array("Mattermost posts returned by the API in response order.", postSchema),
      order: s.array("Mattermost post IDs in response order.", idSchema),
      raw: rawResponseSchema,
    }),
  }),
  defineProviderAction(service, {
    name: "create_post",
    description: "Create a Mattermost post in a channel.",
    inputSchema: s.object(
      "Input parameters for creating a Mattermost channel post.",
      {
        channelId: idSchema,
        message: s.string("The Markdown message body to post.", { minLength: 1 }),
        rootId: s.string("Optional root post ID for replying in a thread.", { minLength: 1 }),
        props: s.looseObject("Optional Mattermost post props object."),
      },
      { optional: ["rootId", "props"] },
    ),
    outputSchema: s.object("Output payload for a created Mattermost post.", {
      post: postSchema,
      raw: rawResponseSchema,
    }),
  }),
];
