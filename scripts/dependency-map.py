#!/usr/bin/env python3.11
"""Allternit dependency map: products, surfaces and code, with evidence. Python 3.11+, no installs.

  python3.11 scripts/dependency-map.py --impact <path or id>   what a change can reach (always live)
  python3.11 scripts/dependency-map.py --validate              check runtime-links.json, products.json and features.json
  python3.11 scripts/dependency-map.py --out <dir>             build the viewer (index.html + graph.json)

The built output lists every file in the private allternit-ai repo, so it is never committed to this
public repo. It is published to the access-controlled admin site by scripts/build-admin-site.sh.
"""
import argparse, collections, datetime, json, os, pathlib, re, subprocess, sys, tomllib
ROOT = pathlib.Path(__file__).resolve().parents[1]
MAP = ROOT / 'docs/dependency-map'
SKIP = {'.agents', 'node_modules', 'target', 'dist', 'release', 'vendor', 'archive', '.git', 'resources', '.gw-target', '.shared-target'}

# Packages too large to review as one box: {package node: (source folder, label prefix, max files per group)}.
# Folders are split until each group is under the limit. In Rust crates, loose files in src/ are grouped by
# their first name segment (channel_slack_app.rs and channel_teams_app.rs -> 'channel_*').
SPLIT = {'allternit/cmd/gizzi-code': ('src', 'gizzi-code', 250),
         'allternit/cmd/allternit-api': ('src', 'allternit-api', 60),
         'allternit/cmd/allternit-cloud-api': ('src', 'cloud-api', 40)}

# Evidence outside the two code checkouts. Checked when the folder exists (Eoj's Mac); skipped in CI.
EXTERNAL = {'brain': pathlib.Path.home() / 'Desktop/Allternit/Allternit Brain',
            'websites': pathlib.Path.home() / 'Desktop/Allternit/Allternit Websites'}

STATUSES = ('live', 'building', 'planned')

LAYERS = ['product', 'surface', 'workspace-ui', 'runtime', 'platform', 'domain', 'other']

def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()

def layer(n):
    """Coarse architecture layer used by the viewer's colours and columns."""
    i = n['id']
    if n['kind'] in ('product', 'line', 'feature', 'journey'): return 'product'
    if n['kind'] == 'surface' or i.startswith('allternit/surfaces/'): return 'surface'
    if i.startswith('allternit-ai'): return 'workspace-ui'
    if i.startswith(('allternit/cmd/', 'allternit/services/')): return 'runtime'
    if i.startswith(('allternit/platform/', 'allternit/packages/', 'allternit/sdk/', 'allternit/mcp/')): return 'platform'
    if i.startswith(('allternit/domains/', 'allternit/infrastructure/', 'allternit/drivers/', 'allternit/factory/')): return 'domain'
    return 'other'

def generate(ai, strict=True):
    nodes, edges, files, texts, warnings = {}, {}, {}, {}, []
    roots = {'allternit': ROOT, 'allternit-ai': ai}
    def node(id, label, kind, path, **meta):
        nodes.setdefault(id, dict(id=id, label=label, kind=kind, path=path, **meta))
    def edge(a, b, kind, evidence):
        if a == b: return
        k = (a,b,kind)
        edges.setdefault(k, dict(source=a,target=b,kind=kind,evidence=[]))
        if evidence not in edges[k]['evidence']: edges[k]['evidence'].append(evidence)
    def check_evidence(ev, where):
        prefix, _, rel = ev.split(':')[0].partition('/')
        if prefix in roots: base = roots[prefix]
        elif prefix in EXTERNAL:
            base = EXTERNAL[prefix]
            if not base.is_dir(): return
        else: raise SystemExit(f'{where}: evidence must start with allternit/, allternit-ai/, brain/ or websites/: {ev}')
        if not (base / rel).exists(): raise SystemExit(f'{where}: evidence path does not exist: {ev}')

    manifests, packages, cargo = {}, collections.defaultdict(list), {}
    for repo, root in roots.items():
        if not root.is_dir(): raise SystemExit('Missing checkout: ' + str(root) + ' (pass --ai for allternit-ai)')
        paths = git(root, 'ls-files', '-z', '--cached', '--others', '--exclude-standard').split('\0')
        for rel in sorted(set(paths)):
            p = pathlib.PurePosixPath(rel)
            if not rel or any(x in SKIP or x.startswith('.hosted') for x in p.parts): continue
            full = root / rel
            if not full.is_file() or full.is_symlink(): continue
            if p.name not in ('package.json', 'Cargo.toml') and p.suffix not in ('.ts','.tsx','.js','.jsx','.mjs','.cjs','.rs'): continue
            key = repo + '/' + rel
            texts[key] = full.read_text(errors='replace')
            if p.name in ('package.json','Cargo.toml'):
                try: data = json.loads(texts[key]) if p.name == 'package.json' else tomllib.loads(texts[key])
                except Exception as ex:
                    warnings.append(key + ': ' + str(ex)); continue
                directory = str(p.parent)
                id = repo + ('/' + directory if directory != '.' else '')
                label = data.get('name') if p.name == 'package.json' else data.get('package',{}).get('name')
                node(id, label or id, 'package' if label else 'workspace', id)
                manifests[key] = (id, data, p.name)
                if label: packages[label].append(id)
                if p.name == 'Cargo.toml': cargo[id] = data

    # Associate source with its nearest manifest; split the shared UI and large packages into feature folders.
    dircount = collections.Counter(str(d) for key in texts for d in pathlib.PurePosixPath(key).parents)
    manifest_ids = list(nodes)  # owners come from manifests only, never from modules created below
    for key in texts:
        repo, rel = key.split('/',1)
        p = pathlib.PurePosixPath(rel)
        candidates = [n for n in manifest_ids if key.startswith(n + '/')]
        owner = max(candidates, key=len) if candidates else repo
        if repo == 'allternit-ai' and len(p.parts) >= 3 and p.parts[0] == 'src':
            count = 3 if p.parts[1] in ('views','lib','components') and len(p.parts) > 3 else 2
            owner = repo + '/' + '/'.join(p.parts[:count])
            node(owner, '/'.join(p.parts[1:count]), 'module', owner)
            edge(repo, owner, 'contains', key)
        sub = key[len(owner)+1:].split('/') if owner in SPLIT else []
        if len(sub) >= 2 and sub[0] == SPLIT[owner][0]:
            limit = SPLIT[owner][2]
            size = lambda d: dircount[owner + '/' + '/'.join(sub[:d])]
            # Descend while the folder is still too big to be a useful review unit.
            d = 1
            while d < len(sub) - 1 and size(d) > limit: d += 1
            if d < len(sub) - 1: parts = sub[:max(d, 2)]
            elif size(d) > limit and sub[-1].endswith('.rs'): parts = sub[:d] + [sub[-1][:-3].split('_')[0] + '_*']
            elif d >= 2: parts = sub[:d]
            else: parts = None
            if parts:
                module = owner + '/' + '/'.join(parts)
                node(module, SPLIT[owner][1] + ' · ' + '/'.join(parts[1:]), 'module', module)
                edge(owner, module, 'contains', key)
                owner = module
        node(owner, owner, 'area', owner)
        files[key] = owner

    for key, (owner,data,typ) in manifests.items():
        repo, rel = key.split('/',1)
        if typ == 'package.json':
            for section in ('dependencies','devDependencies','optionalDependencies','peerDependencies'):
                for name,version in data.get(section,{}).items():
                    targets = packages.get(name, [])
                    if str(version).startswith(('file:','link:')):
                        target = ((roots[repo] / rel).parent / version.split(':',1)[1]).resolve()
                        targets = [repo + '/' + str(target.relative_to(roots[repo]))] if target.is_relative_to(roots[repo]) else []
                    for target in targets:
                        if target in nodes: edge(owner,target,section,key)
        else:
            tables = [(k,v) for k,v in data.items() if k.endswith('dependencies')]
            for cfg in data.get('target',{}).values(): tables += [(k,v) for k,v in cfg.items() if k.endswith('dependencies')]
            for section, deps in tables:
                for name,spec in deps.items():
                    if not isinstance(spec,dict): continue
                    inherited = spec.get('workspace')
                    if inherited: spec = cargo.get(repo,{}).get('workspace',{}).get('dependencies',{}).get(name,{})
                    if not isinstance(spec,dict) or 'path' not in spec: continue
                    base = roots[repo] if inherited else (roots[repo] / rel).parent
                    resolved = (base/spec['path']).resolve()
                    if resolved.is_relative_to(roots[repo]):
                        target = repo + '/' + str(resolved.relative_to(roots[repo]))
                        if target in nodes: edge(owner,target,section,key)

    imports = re.compile(r'''(?:from\s*|import\s*\(|require\s*\(|import\s*)["']([^"']+)["']''')
    for key, content in texts.items():
        if not key.endswith(('.ts','.tsx','.js','.jsx','.mjs','.cjs')): continue
        repo, rel = key.split('/',1)
        for match in imports.finditer(content):
            spec = match.group(1)
            if spec.startswith('.') or (repo == 'allternit-ai' and spec.startswith('@/')):
                base = roots[repo] / ('src/' + spec[2:] if spec.startswith('@/') else str(pathlib.PurePosixPath(rel).parent / spec))
                for suffix in ('','.ts','.tsx','.js','.jsx','/index.ts','/index.tsx','/index.js'):
                    target_path = pathlib.Path(os.path.abspath(str(base)+suffix))
                    if not target_path.is_relative_to(roots[repo]): continue
                    target = repo + '/' + str(target_path.relative_to(roots[repo]))
                    if target in files:
                        edge(files[key],files[target],'import',key); break
            else:
                name = '/'.join(spec.split('/')[:2]) if spec.startswith('@') else spec.split('/')[0]
                for target in packages.get(name,[]): edge(files[key],target,'import',key)

    # Rust: `crate::name` inside a split crate links to the group that defines `name`.
    crate_ref = re.compile(r'\bcrate::(\w+)')
    for key, content in texts.items():
        crate = next((c for c in SPLIT if key.startswith(c + '/') and key.endswith('.rs')), None)
        if not crate: continue
        src = crate + '/' + SPLIT[crate][0] + '/'
        for name in set(crate_ref.findall(content)):
            for target in (src + name + '.rs', src + name + '/mod.rs', src + name + '/lib.rs'):
                if target in files:
                    edge(files[key], files[target], 'crate ref', key); break

    # Curated: runtime calls, sidecars and shipping that imports cannot show.
    manual = json.loads((MAP/'runtime-links.json').read_text())
    for n in manual['nodes']: node(n['id'],n['label'],n['kind'],n['path'])
    for e in manual['edges']:
        for end in (e['source'], e['target']):
            if end not in nodes: raise SystemExit(f'runtime-links.json: unknown component {end!r}')
        for ev in e['evidence']:
            check_evidence(ev, 'runtime-links.json')
            edge(e['source'],e['target'],e['kind'],ev)

    # Curated: product lines and products, layered over the code.
    catalog = json.loads((MAP/'products.json').read_text())
    for l in catalog['lines']: node(l['id'], l['label'], 'line', '', description=l['description'])
    for pr in catalog['products']:
        node(pr['id'], pr['label'], 'product', pr.get('path',''), type=pr['type'], description=pr['description'],
             url=pr.get('url'), line=pr['line'], repo=pr.get('repo'))
    for pr in catalog['products']:
        where = 'products.json ' + pr['id']
        for ev in pr['evidence']: check_evidence(ev, where)
        ev = pr['evidence'][0] if pr['evidence'] else 'docs/dependency-map/products.json'
        if pr['line'] not in nodes: raise SystemExit(f'{where}: unknown line {pr["line"]!r}')
        edge(pr['line'], pr['id'], 'includes', 'allternit/docs/dependency-map/products.json')
        for kind, refs in (('ships on', pr.get('surfaces', [])), ('built from', pr.get('components', [])), ('uses product', pr.get('uses', []))):
            for ref in refs:
                if ref not in nodes: raise SystemExit(f'{where}: unknown {kind} reference {ref!r}')
                edge(pr['id'], ref, kind, ev)

    # Curated: features (what a product does and the components it touches) and journeys (ordered steps a user
    # takes across components). Edges point from the feature or journey to the component, like every other edge.
    # Strict only under --validate: an agent's allternit-ai checkout may be behind main, and a component a feature
    # names may not exist there yet. --impact and the site build then warn and skip that reference.
    def stale(msg):
        if strict: raise SystemExit(msg)
        warnings.append(msg + ' (skipped; run --validate against current main)')
    work = json.loads((MAP/'features.json').read_text())
    for f in work['features']:
        where = 'features.json ' + f['id']
        if not f['id'].startswith('feature:'): raise SystemExit(f'{where}: feature ids start with feature:')
        if nodes.get(f['product'], {}).get('kind') != 'product': raise SystemExit(f'{where}: unknown product {f["product"]!r}')
        if f['status'] not in STATUSES: raise SystemExit(f'{where}: status must be one of {", ".join(STATUSES)}')
        if not f['touches']: raise SystemExit(f'{where}: list at least one component in touches')
        for d in f.get('decisions', []):
            try: datetime.date.fromisoformat(d['on'])
            except (KeyError, ValueError): raise SystemExit(f'{where}: decision dates are YYYY-MM-DD: {d!r}')
            if not d.get('note', '').strip(): raise SystemExit(f'{where}: decision on {d["on"]} has no note')
        node(f['id'], f['label'], 'feature', '', description=f['description'], product=f['product'], status=f['status'],
             decisions=sorted(f.get('decisions', []), key=lambda d: d['on'], reverse=True))
    for j in work['journeys']:
        where = 'features.json ' + j['id']
        if not j['id'].startswith('journey:'): raise SystemExit(f'{where}: journey ids start with journey:')
        if len(j['steps']) < 2: raise SystemExit(f'{where}: a journey needs at least two steps')
        node(j['id'], j['label'], 'journey', '', description=j['description'], steps=[[s['at'], s['says']] for s in j['steps']])
    for item in work['features'] + work['journeys']:
        where = 'features.json ' + item['id']
        if not item['evidence']: raise SystemExit(f'{where}: add at least one evidence path')
        for ev in item['evidence']:
            try: check_evidence(ev, where)
            except SystemExit as ex: stale(str(ex))
        refs = [('touches', r) for r in item.get('touches', [])] + [('step', s['at']) for s in item.get('steps', [])]
        for kind, ref in refs:
            if ref not in nodes or nodes[ref]['kind'] in ('feature', 'journey', 'line'):
                stale(f'{where}: unknown {kind} component {ref!r}'); continue
            edge(item['id'], ref, kind, item['evidence'][0])
        if item['id'].startswith('feature:'): edge(item['product'], item['id'], 'has feature', item['evidence'][0])

    for n in nodes.values(): n['layer'] = layer(n)
    return dict(schema=2, revisions={r:git(p,'rev-parse','HEAD') for r,p in roots.items()},
        nodes=sorted(nodes.values(),key=lambda n:n['id']), edges=sorted(edges.values(),key=lambda e:(e['source'],e['target'],e['kind'])),
        files=files, warnings=warnings)

def viewer_data(g, built):
    """Compact copy for index.html: integer node refs, evidence capped per edge, files grouped by owner."""
    index = {n['id']: i for i, n in enumerate(g['nodes'])}
    owners = collections.defaultdict(list)
    for path, owner in g['files'].items(): owners[index[owner]].append(path)
    keys = ('type','description','url','line','repo','product','status','decisions')
    meta = {str(index[n['id']]): {k: n[k] for k in keys if n.get(k)} for n in g['nodes'] if n['kind'] in ('product','line','feature','journey')}
    for n in g['nodes']:
        if n['kind'] == 'journey': meta[str(index[n['id']])]['steps'] = [[index[at], says] for at, says in n['steps'] if at in index]
    return dict(revisions=g['revisions'], built=built, layers=LAYERS,
        nodes=[[n['id'], n['label'], n['kind'], n['layer']] for n in g['nodes']], meta=meta,
        edges=[[index[e['source']], index[e['target']], e['kind'], e['evidence'][:4], len(e['evidence'])] for e in g['edges']],
        files={str(k): v for k, v in sorted(owners.items())}, fileCount=len(g['files']))

def render(g, built):
    data = json.dumps(viewer_data(g, built), separators=(',', ':')).replace('<', '\\u003c')
    return (MAP/'viewer.template.html').read_text().replace('__GRAPH_DATA__', data)

def impact(g, query):
    ids = {n['id'] for n in g['nodes']}
    seed = {query} if query in ids else {v for k,v in g['files'].items() if k == query or k.startswith(query.rstrip('/')+'/')}
    if not seed: raise SystemExit('No mapped path: '+query)
    consumers = collections.defaultdict(list)
    for e in g['edges']: consumers[e['target']].append(e['source'])
    seen=set(seed); todo=list(seed)
    while todo:
        for source in consumers[todo.pop()]:
            if source not in seen: seen.add(source); todo.append(source)
    kinds = {n['id']: n['kind'] for n in g['nodes']}
    # Part of: products that list the changed component, or a folder/crate containing it, as built from.
    up = set(seed); todo = list(seed)
    while todo:
        x = todo.pop()
        for e in g['edges']:
            if e['kind'] == 'contains' and e['target'] == x and e['source'] not in up: up.add(e['source']); todo.append(e['source'])
    part_of = sorted({e['source'] for e in g['edges'] if e['kind'] == 'built from' and e['target'] in up})
    # Features and journeys that name the changed component, or a folder/crate containing it, directly.
    named = {e['source'] for e in g['edges'] if e['kind'] in ('touches', 'step') and e['target'] in up}
    return {'changed':sorted(seed), 'partOf':part_of,
            'products':sorted(x for x in seen if kinds[x]=='product'),
            'surfaces':sorted(x for x in seen if kinds[x]=='surface'),
            'features':sorted({x for x in named if kinds[x]=='feature'}),
            'journeys':sorted({x for x in named if kinds[x]=='journey'}),
            'potentiallyAffected':sorted(seen-seed)}

if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--ai',type=pathlib.Path,default=ROOT.parent/'allternit-ai',help='allternit-ai checkout (default: sibling folder)')
    p.add_argument('--impact',help='Repo-prefixed file, directory, or component id')
    p.add_argument('--validate',action='store_true',help='Check the curated files resolve, then exit')
    p.add_argument('--out',type=pathlib.Path,help='Write index.html and graph.json into this folder')
    p.add_argument('--built',default='',help='Build timestamp shown in the viewer')
    a=p.parse_args(); g=generate(a.ai.resolve(), strict=not (a.impact or a.out))
    if a.impact: print(json.dumps(impact(g,a.impact),indent=2))
    elif a.out:
        a.out.mkdir(parents=True, exist_ok=True)
        (a.out/'graph.json').write_text(json.dumps(g,indent=1)+'\n')
        (a.out/'index.html').write_text(render(g, a.built))
        print(f"{len(g['nodes'])} components, {len(g['edges'])} relationships, {len(g['files'])} files -> {a.out}")
    else:
        count = collections.Counter(n['kind'] for n in g['nodes'])
        print(f"Valid: {count['product']} products, {count['feature']} features, {count['journey']} journeys, "
              f"{len(g['nodes'])} components, {len(g['edges'])} relationships, {len(g['files'])} files.")
    if g['warnings']: print('\n'.join(g['warnings']), file=sys.stderr)
