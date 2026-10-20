import { type Client, type ClientOptions } from "./client/index.js";
import { agentCreate, agentGet, agentList, agentUpdate, appSkills, arsContextaEnrich, arsContextaEntities, arsContextaHealth, arsContextaInsights, arsContextaProviders, assetDelete, assetGet, assetList, assetUpload, authRemove, authSet, commandList, configGet, configProviders, configUpdate, cronAllRuns, cronCleanupSession, cronCreate, cronDelete, cronGet, cronGetRun, cronList, cronPause, cronResume, cronRun, cronRuns, cronStatus, cronUpdate, cronWake, engineExecute, engineHealth, engineReceipts, engineRunApproval, engineRunCancel, engineRunEvents, engineRunGet, engineRunPause, engineRunResume, engineSnapshot, engineWatch, eventSubscribe, fileGlob, fileInfo, fileRead, fileSearch, fileSymbols, fileTree, filesDelete, filesGet, filesList, filesUpload, formatterStatus, getV1MemorySearch, globalEvent, globalHealth, globalVersion, instanceDispose, instanceHealth, instanceVersion, instanceWorkspace, lspStatus, mcpAdd, mcpList, mcpRemove, mcpResources, mcpStatus, pathGet, permissionList, permissionReply, postV1PluginInstall, postV1PluginRemove, projectAgentList, projectFindFile, projectGet, projectInit, projectList, projectListRoot, projectSessionAbort, projectSessionCompact, projectSessionCreate, projectSessionDelete, projectSessionFileFind, projectSessionFileRead, projectSessionFileStatus, projectSessionGet, projectSessionInitialize, projectSessionList, projectSessionMessageCreate, projectSessionMessageGet, projectSessionMessages, projectSessionPermissionReply, projectSessionRevert, projectSessionShare, projectSessionUnrevert, projectSessionUnshare, projectUpdate, providerAuth, providerList, providerOauthAuthorize, providerOauthVerify, ptyCreate, ptyGet, ptyKill, ptyList, putV1MemoryByFilename, putV1MemoryL1BySessionId, putV1MemoryL2ByType, questionList, questionReject, questionReply, sandboxDisable, sandboxEnable, sandboxGet, sandboxPolicy, sandboxToggle, sessionAbort, sessionAllStatus, sessionChildren, sessionTasksList, sessionBackgroundTasksList, sessionBackgroundTasksGet, sessionBackgroundTasksCancel, sessionCommand, sessionCreate, sessionDelete, sessionDiff, sessionFork, sessionGet, sessionInitialize, sessionList, sessionListGlobal, sessionMessages, sessionPrompt, sessionRevert, sessionShare, sessionSummarize, sessionTodo, sessionUnrevert, sessionUpdate, skillAdd, skillEval, skillEvalsGet, skillEvalsList, skillInstall, skillPublish, skillRegistry, skillToolIds, skillTools, terminalClerkCallback, terminalClerkClaim, terminalClerkPoll, terminalClerkStart, tokensCount, tuiAppendPrompt, tuiClearPrompt, tuiControlNext, tuiControlResponse, tuiExecuteCommand, tuiOpenHelp, tuiOpenModels, tuiOpenSessions, tuiOpenThemes, tuiPublish, tuiSelectSession, tuiShowToast, tuiSubmitPrompt, userClear, userGet, userOnboard, userRefresh, vcsGet, vcsWorktreeCreate, vcsWorktreeRemove, vmSessionDestroy, vmSessionDisable, vmSessionEnable, vmSessionGet, vmSessionToggle, workspaceActivate, workspaceGet, workspaceIdentityGet, workspaceIdentityPut, workspaceImport, workspaceInit, workspaceLayers, workspaceMemoryGet, workspaceMemoryPost, workspaceSkills } from "./sdk.gen.js";
declare class HeyApiClient {
    protected client: Client;
    constructor(args?: {
        client?: Client;
    });
}
declare class HeyApiRegistry<T> {
    private readonly defaultKey;
    private readonly instances;
    get(key?: string): T;
    set(value: T, key?: string): void;
}
declare class ArsContexta extends HeyApiClient {
    enrich(options?: Parameters<typeof arsContextaEnrich>[0]): any;
    entities(options?: Parameters<typeof arsContextaEntities>[0]): any;
    health(options?: Parameters<typeof arsContextaHealth>[0]): any;
    insights(options?: Parameters<typeof arsContextaInsights>[0]): any;
    providers(options?: Parameters<typeof arsContextaProviders>[0]): any;
}
declare class CronCleanup extends HeyApiClient {
    session(options?: Parameters<typeof cronCleanupSession>[0]): any;
}
declare class EngineRun extends HeyApiClient {
    approval(options?: Parameters<typeof engineRunApproval>[0]): any;
    cancel(options?: Parameters<typeof engineRunCancel>[0]): any;
    events(options?: Parameters<typeof engineRunEvents>[0]): any;
    get(options?: Parameters<typeof engineRunGet>[0]): any;
    pause(options?: Parameters<typeof engineRunPause>[0]): any;
    resume(options?: Parameters<typeof engineRunResume>[0]): any;
}
declare class ProjectAgent extends HeyApiClient {
    list(options?: Parameters<typeof projectAgentList>[0]): any;
}
declare class ProjectFind extends HeyApiClient {
    file(options?: Parameters<typeof projectFindFile>[0]): any;
}
declare class ProjectSession extends HeyApiClient {
    abort(options?: Parameters<typeof projectSessionAbort>[0]): any;
    compact(options?: Parameters<typeof projectSessionCompact>[0]): any;
    create(options?: Parameters<typeof projectSessionCreate>[0]): any;
    delete(options?: Parameters<typeof projectSessionDelete>[0]): any;
    get(options?: Parameters<typeof projectSessionGet>[0]): any;
    initialize(options?: Parameters<typeof projectSessionInitialize>[0]): any;
    list(options?: Parameters<typeof projectSessionList>[0]): any;
    messages(options?: Parameters<typeof projectSessionMessages>[0]): any;
    revert(options?: Parameters<typeof projectSessionRevert>[0]): any;
    share(options?: Parameters<typeof projectSessionShare>[0]): any;
    unrevert(options?: Parameters<typeof projectSessionUnrevert>[0]): any;
    unshare(options?: Parameters<typeof projectSessionUnshare>[0]): any;
}
declare class ProviderOauth extends HeyApiClient {
    authorize(options?: Parameters<typeof providerOauthAuthorize>[0]): any;
    verify(options?: Parameters<typeof providerOauthVerify>[0]): any;
    callback(options?: Parameters<typeof providerOauthVerify>[0]): any;
}
declare class SkillEvals extends HeyApiClient {
    get(options?: Parameters<typeof skillEvalsGet>[0]): any;
    list(options?: Parameters<typeof skillEvalsList>[0]): any;
}
declare class SkillTool extends HeyApiClient {
    ids(options?: Parameters<typeof skillToolIds>[0]): any;
}
declare class TerminalClerk extends HeyApiClient {
    callback(options?: Parameters<typeof terminalClerkCallback>[0]): any;
    claim(options?: Parameters<typeof terminalClerkClaim>[0]): any;
    poll(options?: Parameters<typeof terminalClerkPoll>[0]): any;
    start(options?: Parameters<typeof terminalClerkStart>[0]): any;
}
declare class TuiAppend extends HeyApiClient {
    prompt(options?: Parameters<typeof tuiAppendPrompt>[0]): any;
}
declare class TuiClear extends HeyApiClient {
    prompt(options?: Parameters<typeof tuiClearPrompt>[0]): any;
}
declare class TuiControl extends HeyApiClient {
    next(options?: Parameters<typeof tuiControlNext>[0]): any;
    response(options?: Parameters<typeof tuiControlResponse>[0]): any;
}
declare class TuiExecute extends HeyApiClient {
    command(options?: Parameters<typeof tuiExecuteCommand>[0]): any;
}
declare class TuiOpen extends HeyApiClient {
    help(options?: Parameters<typeof tuiOpenHelp>[0]): any;
    models(options?: Parameters<typeof tuiOpenModels>[0]): any;
    sessions(options?: Parameters<typeof tuiOpenSessions>[0]): any;
    themes(options?: Parameters<typeof tuiOpenThemes>[0]): any;
}
declare class TuiSelect extends HeyApiClient {
    session(options?: Parameters<typeof tuiSelectSession>[0]): any;
}
declare class TuiShow extends HeyApiClient {
    toast(options?: Parameters<typeof tuiShowToast>[0]): any;
}
declare class TuiSubmit extends HeyApiClient {
    prompt(options?: Parameters<typeof tuiSubmitPrompt>[0]): any;
}
declare class VcsWorktree extends HeyApiClient {
    create(options?: Parameters<typeof vcsWorktreeCreate>[0]): any;
    remove(options?: Parameters<typeof vcsWorktreeRemove>[0]): any;
}
declare class VmSession extends HeyApiClient {
    destroy(options?: Parameters<typeof vmSessionDestroy>[0]): any;
    disable(options?: Parameters<typeof vmSessionDisable>[0]): any;
    enable(options?: Parameters<typeof vmSessionEnable>[0]): any;
    get(options?: Parameters<typeof vmSessionGet>[0]): any;
    toggle(options?: Parameters<typeof vmSessionToggle>[0]): any;
}
declare class WorkspaceIdentity extends HeyApiClient {
    get(options?: Parameters<typeof workspaceIdentityGet>[0]): any;
    put(options?: Parameters<typeof workspaceIdentityPut>[0]): any;
}
declare class WorkspaceMemory extends HeyApiClient {
    get(options?: Parameters<typeof workspaceMemoryGet>[0]): any;
    post(options?: Parameters<typeof workspaceMemoryPost>[0]): any;
}
declare class Agent extends HeyApiClient {
    create(options?: Parameters<typeof agentCreate>[0]): any;
    get(options?: Parameters<typeof agentGet>[0]): any;
    list(options?: Parameters<typeof agentList>[0]): any;
    update(options?: Parameters<typeof agentUpdate>[0]): any;
}
declare class App extends HeyApiClient {
    agents(options?: Parameters<typeof agentList>[0]): any;
    skills(options?: Parameters<typeof appSkills>[0]): any;
}
declare class Ars extends HeyApiClient {
    private _contexta?;
    get contexta(): ArsContexta;
}
declare class Asset extends HeyApiClient {
    delete(options?: Parameters<typeof assetDelete>[0]): any;
    get(options?: Parameters<typeof assetGet>[0]): any;
    list(options?: Parameters<typeof assetList>[0]): any;
    upload(options?: Parameters<typeof assetUpload>[0]): any;
}
declare class Auth extends HeyApiClient {
    remove(options?: Parameters<typeof authRemove>[0]): any;
    set(options?: Parameters<typeof authSet>[0]): any;
}
declare class Command extends HeyApiClient {
    list(options?: Parameters<typeof commandList>[0]): any;
}
declare class Config extends HeyApiClient {
    get(options?: Parameters<typeof configGet>[0]): any;
    providers(options?: Parameters<typeof configProviders>[0]): any;
    update(options?: Parameters<typeof configUpdate>[0]): any;
}
declare class Cron extends HeyApiClient {
    allruns(options?: Parameters<typeof cronAllRuns>[0]): any;
    create(options?: Parameters<typeof cronCreate>[0]): any;
    delete(options?: Parameters<typeof cronDelete>[0]): any;
    get(options?: Parameters<typeof cronGet>[0]): any;
    getrun(options?: Parameters<typeof cronGetRun>[0]): any;
    list(options?: Parameters<typeof cronList>[0]): any;
    pause(options?: Parameters<typeof cronPause>[0]): any;
    resume(options?: Parameters<typeof cronResume>[0]): any;
    run(options?: Parameters<typeof cronRun>[0]): any;
    runs(options?: Parameters<typeof cronRuns>[0]): any;
    status(options?: Parameters<typeof cronStatus>[0]): any;
    update(options?: Parameters<typeof cronUpdate>[0]): any;
    wake(options?: Parameters<typeof cronWake>[0]): any;
    private _cleanup?;
    get cleanup(): CronCleanup;
}
declare class Engine extends HeyApiClient {
    execute(options?: Parameters<typeof engineExecute>[0]): any;
    health(options?: Parameters<typeof engineHealth>[0]): any;
    receipts(options?: Parameters<typeof engineReceipts>[0]): any;
    snapshot(options?: Parameters<typeof engineSnapshot>[0]): any;
    watch(options?: Parameters<typeof engineWatch>[0]): any;
    private _run?;
    get run(): EngineRun;
}
declare class Event extends HeyApiClient {
    /** Typed async iterator over server-sent events. */
    stream(options?: {
        signal?: AbortSignal;
    }): AsyncIterableIterator<import('./entity-types.js').Event>;
    subscribe(options?: Parameters<typeof eventSubscribe>[0]): any;
}
declare class File extends HeyApiClient {
    glob(options?: Parameters<typeof fileGlob>[0]): any;
    info(options?: Parameters<typeof fileInfo>[0]): any;
    read(options?: Parameters<typeof fileRead>[0]): any;
    search(options?: Parameters<typeof fileSearch>[0]): any;
    symbols(options?: Parameters<typeof fileSymbols>[0]): any;
    tree(options?: Parameters<typeof fileTree>[0]): any;
}
declare class Files extends HeyApiClient {
    delete(options?: Parameters<typeof filesDelete>[0]): any;
    get(options?: Parameters<typeof filesGet>[0]): any;
    list(options?: Parameters<typeof filesList>[0]): any;
    upload(options?: Parameters<typeof filesUpload>[0]): any;
}
declare class Formatter extends HeyApiClient {
    status(options?: Parameters<typeof formatterStatus>[0]): any;
}
declare class Get extends HeyApiClient {
    v1memorysearch(options?: Parameters<typeof getV1MemorySearch>[0]): any;
}
declare class Global extends HeyApiClient {
    /** Typed async iterator over global server-sent events. */
    stream(options?: {
        signal?: AbortSignal;
    }): AsyncIterableIterator<import('./entity-types.js').Event>;
    event(options?: Parameters<typeof globalEvent>[0]): any;
    health(options?: Parameters<typeof globalHealth>[0]): any;
    version(options?: Parameters<typeof globalVersion>[0]): any;
}
declare class Instance extends HeyApiClient {
    dispose(options?: Parameters<typeof instanceDispose>[0]): any;
    health(options?: Parameters<typeof instanceHealth>[0]): any;
    version(options?: Parameters<typeof instanceVersion>[0]): any;
    workspace(options?: Parameters<typeof instanceWorkspace>[0]): any;
}
declare class Lsp extends HeyApiClient {
    status(options?: Parameters<typeof lspStatus>[0]): any;
}
declare class Mcp extends HeyApiClient {
    add(options?: Parameters<typeof mcpAdd>[0]): any;
    list(options?: Parameters<typeof mcpList>[0]): any;
    remove(options?: Parameters<typeof mcpRemove>[0]): any;
    resources(options?: Parameters<typeof mcpResources>[0]): any;
    status(options?: Parameters<typeof mcpStatus>[0]): any;
}
declare class Path extends HeyApiClient {
    get(options?: Parameters<typeof pathGet>[0]): any;
}
declare class Permission extends HeyApiClient {
    list(options?: Parameters<typeof permissionList>[0]): any;
    reply(options?: Parameters<typeof permissionReply>[0]): any;
}
declare class Post extends HeyApiClient {
    v1plugininstall(options?: Parameters<typeof postV1PluginInstall>[0]): any;
    v1pluginremove(options?: Parameters<typeof postV1PluginRemove>[0]): any;
}
declare class Project extends HeyApiClient {
    get(options?: Parameters<typeof projectGet>[0]): any;
    init(options?: Parameters<typeof projectInit>[0]): any;
    list(options?: Parameters<typeof projectList>[0]): any;
    listroot(options?: Parameters<typeof projectListRoot>[0]): any;
    sessionfilefind(options?: Parameters<typeof projectSessionFileFind>[0]): any;
    sessionfileread(options?: Parameters<typeof projectSessionFileRead>[0]): any;
    sessionfilestatus(options?: Parameters<typeof projectSessionFileStatus>[0]): any;
    sessionmessagecreate(options?: Parameters<typeof projectSessionMessageCreate>[0]): any;
    sessionmessageget(options?: Parameters<typeof projectSessionMessageGet>[0]): any;
    sessionpermissionreply(options?: Parameters<typeof projectSessionPermissionReply>[0]): any;
    update(options?: Parameters<typeof projectUpdate>[0]): any;
    private _agent?;
    get agent(): ProjectAgent;
    private _find?;
    get find(): ProjectFind;
    private _session?;
    get session(): ProjectSession;
}
declare class Provider extends HeyApiClient {
    auth(options?: Parameters<typeof providerAuth>[0]): any;
    list(options?: Parameters<typeof providerList>[0]): any;
    private _oauth?;
    get oauth(): ProviderOauth;
}
declare class Pty extends HeyApiClient {
    create(options?: Parameters<typeof ptyCreate>[0]): any;
    get(options?: Parameters<typeof ptyGet>[0]): any;
    kill(options?: Parameters<typeof ptyKill>[0]): any;
    list(options?: Parameters<typeof ptyList>[0]): any;
}
declare class Put extends HeyApiClient {
    v1memorybyfilename(options?: Parameters<typeof putV1MemoryByFilename>[0]): any;
    v1memoryl1bysessionid(options?: Parameters<typeof putV1MemoryL1BySessionId>[0]): any;
    v1memoryl2bytype(options?: Parameters<typeof putV1MemoryL2ByType>[0]): any;
}
declare class Question extends HeyApiClient {
    list(options?: Parameters<typeof questionList>[0]): any;
    reject(options?: Parameters<typeof questionReject>[0]): any;
    reply(options?: Parameters<typeof questionReply>[0]): any;
}
declare class Sandbox extends HeyApiClient {
    disable(options?: Parameters<typeof sandboxDisable>[0]): any;
    enable(options?: Parameters<typeof sandboxEnable>[0]): any;
    get(options?: Parameters<typeof sandboxGet>[0]): any;
    policy(options?: Parameters<typeof sandboxPolicy>[0]): any;
    toggle(options?: Parameters<typeof sandboxToggle>[0]): any;
}
declare class Session extends HeyApiClient {
    private convertOptions;
    abort(options?: Parameters<typeof sessionAbort>[0]): any;
    allstatus(options?: Parameters<typeof sessionAllStatus>[0]): any;
    children(options?: Parameters<typeof sessionChildren>[0]): any;
    /** Everything the session delegated: background tasks and foreground subagents. */
    tasks(options?: Parameters<typeof sessionTasksList>[0]): any;
    backgroundTasks(options?: Parameters<typeof sessionBackgroundTasksList>[0]): any;
    backgroundTask(options?: Parameters<typeof sessionBackgroundTasksGet>[0]): any;
    cancelBackgroundTask(options?: Parameters<typeof sessionBackgroundTasksCancel>[0]): any;
    command(options?: Parameters<typeof sessionCommand>[0]): any;
    create(options?: Parameters<typeof sessionCreate>[0]): any;
    delete(options?: Parameters<typeof sessionDelete>[0]): any;
    diff(options?: Parameters<typeof sessionDiff>[0]): any;
    fork(options?: Parameters<typeof sessionFork>[0]): any;
    get(options?: Parameters<typeof sessionGet>[0]): any;
    initialize(options?: Parameters<typeof sessionInitialize>[0]): any;
    list(options?: Parameters<typeof sessionList>[0]): any;
    listglobal(options?: Parameters<typeof sessionListGlobal>[0]): any;
    messages(options?: Parameters<typeof sessionMessages>[0]): any;
    prompt(options?: Parameters<typeof sessionPrompt>[0]): any;
    revert(options?: Parameters<typeof sessionRevert>[0]): any;
    share(options?: Parameters<typeof sessionShare>[0]): any;
    summarize(options?: Parameters<typeof sessionSummarize>[0]): any;
    todo(options?: Parameters<typeof sessionTodo>[0]): any;
    unrevert(options?: Parameters<typeof sessionUnrevert>[0]): any;
    update(options?: Parameters<typeof sessionUpdate>[0]): any;
}
declare class Skill extends HeyApiClient {
    add(options?: Parameters<typeof skillAdd>[0]): any;
    eval(options?: Parameters<typeof skillEval>[0]): any;
    install(options?: Parameters<typeof skillInstall>[0]): any;
    publish(options?: Parameters<typeof skillPublish>[0]): any;
    registry(options?: Parameters<typeof skillRegistry>[0]): any;
    tools(options?: Parameters<typeof skillTools>[0]): any;
    private _evals?;
    get evals(): SkillEvals;
    private _tool?;
    get tool(): SkillTool;
}
declare class Terminal extends HeyApiClient {
    private _clerk?;
    get clerk(): TerminalClerk;
}
declare class Tokens extends HeyApiClient {
    count(options?: Parameters<typeof tokensCount>[0]): any;
}
declare class Tui extends HeyApiClient {
    publish(options?: Parameters<typeof tuiPublish>[0]): any;
    private _append?;
    get append(): TuiAppend;
    private _clear?;
    get clear(): TuiClear;
    private _control?;
    get control(): TuiControl;
    private _execute?;
    get execute(): TuiExecute;
    private _open?;
    get open(): TuiOpen;
    private _select?;
    get select(): TuiSelect;
    private _show?;
    get show(): TuiShow;
    private _submit?;
    get submit(): TuiSubmit;
}
declare class User extends HeyApiClient {
    clear(options?: Parameters<typeof userClear>[0]): any;
    get(options?: Parameters<typeof userGet>[0]): any;
    onboard(options?: Parameters<typeof userOnboard>[0]): any;
    refresh(options?: Parameters<typeof userRefresh>[0]): any;
}
declare class Vcs extends HeyApiClient {
    get(options?: Parameters<typeof vcsGet>[0]): any;
    private _worktree?;
    get worktree(): VcsWorktree;
}
declare class Vm extends HeyApiClient {
    private _session?;
    get session(): VmSession;
}
declare class Workspace extends HeyApiClient {
    activate(options?: Parameters<typeof workspaceActivate>[0]): any;
    get(options?: Parameters<typeof workspaceGet>[0]): any;
    import(options?: Parameters<typeof workspaceImport>[0]): any;
    init(options?: Parameters<typeof workspaceInit>[0]): any;
    layers(options?: Parameters<typeof workspaceLayers>[0]): any;
    skills(options?: Parameters<typeof workspaceSkills>[0]): any;
    private _identity?;
    get identity(): WorkspaceIdentity;
    private _memory?;
    get memory(): WorkspaceMemory;
}
export declare class AllternitClient extends HeyApiClient {
    static readonly __registry: HeyApiRegistry<AllternitClient>;
    constructor(args?: {
        client?: Client;
        key?: string;
    });
    private _agent?;
    get agent(): Agent;
    private _app?;
    get app(): App;
    private _ars?;
    get ars(): Ars;
    private _asset?;
    get asset(): Asset;
    private _auth?;
    get auth(): Auth;
    private _command?;
    get command(): Command;
    private _config?;
    get config(): Config;
    private _cron?;
    get cron(): Cron;
    private _engine?;
    get engine(): Engine;
    private _event?;
    get event(): Event;
    private _file?;
    get file(): File;
    private _files?;
    get files(): Files;
    private _formatter?;
    get formatter(): Formatter;
    private _get?;
    get get(): Get;
    private _global?;
    get global(): Global;
    private _instance?;
    get instance(): Instance;
    private _lsp?;
    get lsp(): Lsp;
    private _mcp?;
    get mcp(): Mcp;
    private _path?;
    get path(): Path;
    private _permission?;
    get permission(): Permission;
    private _post?;
    get post(): Post;
    private _project?;
    get project(): Project;
    private _provider?;
    get provider(): Provider;
    private _pty?;
    get pty(): Pty;
    private _put?;
    get put(): Put;
    private _question?;
    get question(): Question;
    private _sandbox?;
    get sandbox(): Sandbox;
    private _session?;
    get session(): Session;
    private _skill?;
    get skill(): Skill;
    private _terminal?;
    get terminal(): Terminal;
    private _tokens?;
    get tokens(): Tokens;
    private _tui?;
    get tui(): Tui;
    private _user?;
    get user(): User;
    private _vcs?;
    get vcs(): Vcs;
    private _vm?;
    get vm(): Vm;
    private _workspace?;
    get workspace(): Workspace;
    /** Typed async iterator over instance-scoped events (GET /event). */
    events(options?: {
        signal?: AbortSignal;
    }): AsyncIterableIterator<import('./entity-types.js').Event>;
    /** Typed async iterator over global events (GET /global/event). */
    globalEvents(options?: {
        signal?: AbortSignal;
    }): AsyncIterableIterator<import('./entity-types.js').Event>;
    /**
     * Typed SSE subscription filtered to a specific event type.
     * Type of `properties` is automatically narrowed based on `type`.
     *
     * @example
     * for await (const { properties } of sdk.on('session.status')) {
     *   console.log(properties.status) // typed
     * }
     */
    on<T extends import('./types.gen.js').EventSubscribeResponses[200]['type']>(type: T, options?: {
        signal?: AbortSignal;
    }): AsyncIterableIterator<Extract<import('./types.gen.js').EventSubscribeResponses[200], {
        type: T;
    }>>;
}
export declare function createAllternitClient(config?: {
    baseUrl?: string;
    fetch?: typeof fetch;
    headers?: Record<string, string>;
    directory?: string;
    signal?: AbortSignal;
}): AllternitClient;
export * from "./types.gen.js";
export type { Client, ClientOptions };
//# sourceMappingURL=allternit-client.d.ts.map