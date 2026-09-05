#!/usr/bin/env node
// Prove the CI workflow keeps the least-privilege promises KAN-T102
// made: immutable, documented action pins; read-only permissions; no
// secret, token, publication, privileged-event, or planning-artifact
// path; and locked dependency installation.
//
// Every rule judges the document a real YAML parser read, never the
// lines it was written on. Each workflow is parsed once with a pinned
// parser, and a construct this policy cannot judge — a duplicate key,
// an alias, a merge, an anchor, an explicit tag, a second document, or
// a scalar YAML 1.1 and YAML 1.2 disagree about — ends the check
// rather than being scanned past. Scalars are judged after the parser
// decodes them, so a Unicode escape or a line continuation that spells
// `github.token` is the same expression as the plain spelling and
// meets the same allow-list. Where a value has many equivalent
// spellings the scan reduces it to one normal form and judges that
// against an allow-list of safe forms, so an unrecognised spelling is
// refused rather than missed.
//
// The negative probes mutate the text of the real workflow, so every
// rule that judges a workflow's content is proven to bite on realistic
// input rather than trusted to be configured, and each probe asserts
// the shape it stands in for before standing in for it.
//
// Usage: check_workflow_policy.mjs [workflow...] — check specific
// files (the probes use this to judge mutated fixtures). With no
// arguments the committed workflows are checked and the probes run.
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const self = fileURLToPath(import.meta.url)

function fail(message) {
  process.stderr.write(`check-workflow-policy: ${message}\n`)
  process.exit(1)
}

// The parser is a pinned development dependency of this repository, so
// a clean public checkout that installed the locked workspace has it
// and one that did not is told how to.
let YAML
try {
  YAML = await import('yaml')
} catch {
  fail(
    'missing-parser: the pinned yaml parser is not installed.\n' +
      '\n' +
      'Install the locked workspace dependencies with:\n' +
      '    pnpm install --frozen-lockfile\n' +
      'Then re-run the failed just recipe.',
  )
}
const { LineCounter, isMap, isScalar, isSeq, parseAllDocuments, visit } = YAML

const root = execFileSync('git', ['rev-parse', '--show-toplevel'], { encoding: 'utf8' }).trim()
process.chdir(root)

// The only third-party actions the CI is allowed to run. Publication
// and artifact actions are deliberately absent: KAN-T103 owns previews.
const allowedActions = ['actions/checkout', 'actions/setup-node', 'pnpm/action-setup']

// The only GitHub expressions the CI is allowed to evaluate, written in
// the normal form normalizeExpression produces. The list is short on
// purpose: an expression that is not on it is refused, so reaching a
// secret or the workflow token is never a spelling the policy has to
// recognise. Extend the list deliberately, one safe form at a time.
const allowedExpressions = ['github.ref']

// One schema, chosen rather than inherited: YAML 1.2 core, no merge
// keys, no custom tags, and unique keys required. Everything this
// policy refuses below is refused because the parser was asked to
// report it, not because a scan failed to find it.
const parseOptions = {
  version: '1.2',
  schema: 'core',
  merge: false,
  uniqueKeys: true,
  strict: true,
  customTags: [],
}

// The tag directives a document with no %TAG directive of its own has.
const defaultTagDirectives = { '!!': 'tag:yaml.org,2002:' }

// Plain scalars whose meaning differs between YAML 1.1 and YAML 1.2.
// GitHub's parser is not this one, so a value the two schemas read
// differently is refused rather than resolved on the policy's behalf.
// Quoting the value states which one is meant and passes.
const ambiguousPlainScalars = [
  [
    /^(y|Y|yes|Yes|YES|n|N|no|No|NO|on|On|ON|off|Off|OFF)$/,
    'a YAML 1.1 boolean that YAML 1.2 reads as a string',
  ],
  [/^[-+]?0[0-7]+$/, 'a YAML 1.1 octal that YAML 1.2 reads as a decimal'],
  [/^[-+]?0o[0-7]+$/, 'a YAML 1.2 octal that YAML 1.1 reads as a string'],
  [
    /^[-+]?[0-9][0-9_]*(:[0-5]?[0-9])+$/,
    'a YAML 1.1 base-sixty number that YAML 1.2 reads as a string',
  ],
  [
    /^[-+]?[0-9][0-9_]*_[0-9_]*$/,
    'a YAML 1.1 digit-separated number that YAML 1.2 reads as a string',
  ],
]

// The extraction scripts/check_ci_matrix.sh performs to re-run the
// workflow's command matrix from a fresh checkout. The policy holds
// the workflow to it below, so the audit lifts exactly the commands
// the parser sees rather than a folded or flow-style approximation.
const matrixRunExtraction = /^[ \t]*(?:- )?run:[ \t]*/

function firstLine(message) {
  return String(message).split('\n')[0].trim()
}

function scalarOf(node) {
  return isScalar(node) ? node.value : undefined
}

function stringOf(node) {
  const value = scalarOf(node)
  return typeof value === 'string' ? value : undefined
}

// Find a mapping entry by key name. An absent key and a key with an
// empty value are different answers: `pull_request:` declares a
// trigger, and a missing `with:` declares nothing.
function entry(map, key) {
  const pair = map.items.find((item) => isScalar(item.key) && item.key.value === key)
  return pair === undefined ? undefined : pair.value
}

function hasEntry(map, key) {
  return map.items.some((item) => isScalar(item.key) && item.key.value === key)
}

// Refuse a node the parser had to decorate to read: an anchor another
// node could point at, or a tag that overrides the chosen schema.
function rejectDecoration(workflow, node) {
  if (node.anchor) {
    fail(
      `unsupported-construct: ${workflow.name} anchors &${node.anchor} at line ${workflow.lineOf(node)}; the policy resolves no anchors, aliases, or merges`,
    )
  }
  if (node.tag) {
    fail(
      `unsupported-construct: ${workflow.name} tags a node ${node.tag} at line ${workflow.lineOf(node)}; the policy reads one schema`,
    )
  }
}

function rejectAmbiguity(workflow, node) {
  if (node.type !== 'PLAIN') return
  const source = workflow.text.slice(node.range[0], node.range[1])
  for (const [pattern, why] of ambiguousPlainScalars) {
    if (pattern.test(source)) {
      fail(
        `ambiguous-schema-value: ${workflow.name} writes '${source}' unquoted at line ${workflow.lineOf(node)}, ${why}`,
      )
    }
  }
}

// Parse a workflow once and refuse anything the policy cannot judge,
// so every rule below reads a document rather than a guess.
function readWorkflow(file) {
  let text
  try {
    text = readFileSync(file, 'utf8')
  } catch {
    fail(`no workflow at ${file}`)
  }
  const name = file.split('/').pop()
  const lineCounter = new LineCounter()
  const documents = parseAllDocuments(text, { ...parseOptions, lineCounter })

  if (documents.length !== 1) {
    fail(
      `unsupported-construct: ${name} is a ${documents.length}-document stream; a workflow is one document`,
    )
  }
  const doc = documents[0]
  const workflow = {
    name,
    text,
    doc,
    lineOf: (node) => (node && node.range ? lineCounter.linePos(node.range[0]).line : 0),
  }

  for (const error of doc.errors) {
    const marker = error.code === 'DUPLICATE_KEY' ? 'duplicate-key' : 'unreadable-workflow'
    fail(`${marker}: ${name} ${firstLine(error.message)}`)
  }
  for (const warning of doc.warnings) {
    fail(`unsupported-construct: ${name} ${firstLine(warning.message)}`)
  }
  if (doc.directives.yaml.explicit) {
    fail(
      `unsupported-construct: ${name} carries a %YAML ${doc.directives.yaml.version} directive; the policy reads one schema`,
    )
  }
  if (JSON.stringify(doc.directives.tags) !== JSON.stringify(defaultTagDirectives)) {
    fail(`unsupported-construct: ${name} declares a %TAG directive; the policy reads one schema`)
  }
  if (!isMap(doc.contents)) {
    fail(`malformed-workflow: ${name} is not a mapping of workflow keys`)
  }

  // GitHub reads the trigger key as the string `on`; YAML 1.1 would
  // read it as a boolean. It is the one scalar the policy resolves for
  // the ecosystem rather than refusing, and it is resolved by name so
  // no other ambiguous spelling rides along with it.
  const triggerKey = doc.contents.items.find(
    (pair) => isScalar(pair.key) && pair.key.value === 'on',
  )?.key

  visit(doc, {
    Alias: (_key, node) =>
      fail(
        `unsupported-construct: ${name} refers to *${node.source} at line ${workflow.lineOf(node)}; the policy resolves no anchors, aliases, or merges`,
      ),
    Map: (_key, node) => rejectDecoration(workflow, node),
    Seq: (_key, node) => rejectDecoration(workflow, node),
    Scalar: (_key, node) => {
      rejectDecoration(workflow, node)
      if (node !== triggerKey) rejectAmbiguity(workflow, node)
    },
  })

  return workflow
}

// Reduce a GitHub expression to the normal form the allow-list is
// written in. Contexts and function names are case-insensitive, index
// syntax names the same property as dot syntax, and whitespace between
// tokens is insignificant, so every equivalent spelling of one reach —
// secrets.NAME, SECRETS['NAME'], secrets [ "name" ] — collapses onto a
// single string and the allow-list judges meaning, not spelling.
function normalizeExpression(raw) {
  return raw
    .toLowerCase()
    .replace(/\[\s*'([^']*)'\s*\]/g, '.$1')
    .replace(/\[\s*"([^"]*)"\s*\]/g, '.$1')
    .replace(/\s+/g, '')
}

// Name the rule a refused expression breaks. Normalisation has already
// collapsed every spelling, so these are exact tests on one canonical
// string rather than a search through the spellings of a reach.
function classifyExpression(normalized) {
  if (/(^|[^a-z0-9_.])secrets($|[^a-z0-9_])/.test(normalized)) return 'secret-exposure'
  if (/(^|[^a-z0-9_.])github\.token($|[^a-z0-9_])/.test(normalized)) return 'token-exposure'
  return 'unsafe-expression'
}

// Every ${{ }} body inside a decoded scalar. An expression that opens
// without closing ends the scan with a failure: the policy refuses
// what it cannot read rather than reading past it.
function embeddedExpressions(workflow, node, value) {
  const found = []
  let rest = value
  for (;;) {
    const opened = rest.indexOf('${{')
    if (opened < 0) return found
    rest = rest.slice(opened + 3)
    const closed = rest.indexOf('}}')
    if (closed < 0) {
      fail(
        `unterminated-expression: ${workflow.name} opens an expression at line ${workflow.lineOf(node)} that never closes`,
      )
    }
    found.push(rest.slice(0, closed))
    rest = rest.slice(closed + 2)
  }
}

// Every expression the workflow evaluates: each ${{ }} body in every
// decoded scalar, and every `if:` value, which GitHub evaluates as an
// expression whether or not it is wrapped. Delimiters are stripped
// from a conditional so a wrapped, unwrapped or half-wrapped one is
// judged as the single expression it is.
//
// That single rule is the whole secret and token policy: reaching a
// secret or the workflow token is not a spelling to be recognised but
// a form the allow-list never contained, so a whole-context toJSON, a
// nested fromJSON unwrap, a case-insensitive alias, dot syntax, index
// syntax, a Unicode-escaped context name and a continued scalar all
// fail together, and so does any unrecognised expression that arrives
// next.
function checkExpressions(workflow) {
  const evaluated = []
  visit(workflow.doc, {
    Scalar: (_key, node) => {
      if (typeof node.value === 'string') {
        for (const body of embeddedExpressions(workflow, node, node.value)) {
          evaluated.push({ node, source: body })
        }
      }
    },
    Pair: (_key, pair) => {
      if (!isScalar(pair.key) || pair.key.value !== 'if') return
      const condition = String(scalarOf(pair.value) ?? pair.value)
      evaluated.push({ node: pair.value ?? pair.key, source: condition.replace(/\$\{\{|\}\}/g, '') })
    },
  })

  for (const { node, source } of evaluated) {
    const normalized = normalizeExpression(source)
    if (allowedExpressions.includes(normalized)) continue
    fail(
      `${classifyExpression(normalized)}: ${workflow.name} evaluates '${source.trim()}' at line ${workflow.lineOf(node)}, which the expression allow-list does not hold`,
    )
  }
}

// Names that are not expressions but reach the same places: the
// environment variable a run step would read the workflow token from,
// and the ignored temp/ planning artifacts that are never CI inputs.
// Both are judged on decoded scalars, so an escaped spelling is the
// same name.
const forbiddenSubstrings = [
  ['GITHUB_TOKEN', 'token-exposure', 'names the workflow token environment variable'],
  ['temp/', 'planning-artifact-input', 'references temp/'],
  ['project-management', 'planning-artifact-input', 'references project-management'],
  ['check_planning', 'planning-artifact-input', 'references check_planning'],
]

function checkForbiddenNames(workflow) {
  visit(workflow.doc, {
    Scalar: (_key, node) => {
      if (typeof node.value !== 'string') return
      for (const [needle, marker, why] of forbiddenSubstrings) {
        if (node.value.includes(needle)) {
          fail(`${marker}: ${workflow.name} ${why} at line ${workflow.lineOf(node)}`)
        }
      }
    },
  })
}

// Pull requests and main are the only triggers; nothing may hand
// untrusted pull-request code a privileged event context. Pushes gate
// main and nothing else; tags belong to release tickets.
function checkTriggers(workflow) {
  const on = entry(workflow.doc.contents, 'on')
  if (!isMap(on)) {
    fail(`unexpected-trigger: ${workflow.name} declares no pull_request or push trigger mapping`)
  }
  for (const pair of on.items) {
    const event = stringOf(pair.key)
    if (event === 'pull_request' || event === 'push') continue
    if (event === 'pull_request_target' || event === 'workflow_run') {
      fail(
        `privileged-event: ${workflow.name} triggers on ${event}, which untrusted code must never reach`,
      )
    }
    fail(`unexpected-trigger: ${workflow.name} triggers on ${event}, not pull_request or push`)
  }
  if (!hasEntry(on, 'pull_request')) {
    fail(`unexpected-trigger: ${workflow.name} does not run on pull requests`)
  }
  if (!hasEntry(on, 'push')) {
    fail(`unexpected-trigger: ${workflow.name} does not run on pushes`)
  }

  const push = entry(on, 'push')
  if (!isMap(push) || push.items.length !== 1 || !hasEntry(push, 'branches')) {
    fail(`unexpected-push-refs: ${workflow.name} filters pushes by something other than branches`)
  }
  const branches = entry(push, 'branches')
  const refs = isSeq(branches) ? branches.items.map((item) => stringOf(item)) : undefined
  if (refs === undefined || refs.length !== 1 || refs[0] !== 'main') {
    fail(
      `unexpected-push-refs: ${workflow.name} pushes gate [${refs === undefined ? '?' : refs.join(', ')}] instead of [main]`,
    )
  }
}

// Permissions are read-only everywhere: a top-level block must exist,
// state contents: read, and no scope anywhere may grant more. The
// block is judged as the mapping the parser built, so a flow mapping,
// a quoted key, and a block mapping are the same declaration and are
// judged alike, while a string grant such as read-all or write-all is
// not a mapping of scopes at all and is refused.
function checkPermissionsBlock(workflow, node, where) {
  if (!isMap(node)) {
    const written = isScalar(node) ? `'${String(node.value)}'` : 'a value that is not a scope mapping'
    fail(
      `write-permission: ${workflow.name} grants ${where} permissions as ${written}; only a mapping of read or none scopes is accepted`,
    )
  }
  for (const pair of node.items) {
    const scope = stringOf(pair.key)
    const grant = stringOf(pair.value)
    if (scope === undefined || (grant !== 'read' && grant !== 'none')) {
      const written = grant ?? (isScalar(pair.value) ? String(pair.value.value) : 'a value that is not a grant')
      fail(
        `write-permission: ${workflow.name} grants ${where} '${scope ?? 'an unreadable scope'}: ${written}' at line ${workflow.lineOf(pair.value ?? pair.key)}; only 'scope: read' or 'scope: none' is accepted`,
      )
    }
  }
}

function checkPermissions(workflow) {
  const top = workflow.doc.contents
  if (!hasEntry(top, 'permissions')) {
    fail(`write-permission: ${workflow.name} has no top-level read-only permissions block`)
  }
  const permissions = entry(top, 'permissions')
  checkPermissionsBlock(workflow, permissions, 'top-level')
  if (stringOf(entry(permissions, 'contents')) !== 'read') {
    fail(`write-permission: ${workflow.name} never states contents: read`)
  }
}

// Every action must be an allowlisted repository pinned to a full
// commit SHA with its release identity recorded in the comment beside
// the pin, which the parser keeps attached to the value it documents.
function checkActionReference(workflow, node) {
  const reference = stringOf(node)
  if (reference === undefined || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+@[0-9a-f]{40}$/.test(reference)) {
    fail(
      `mutable-action-reference: ${workflow.name} uses ${reference ?? String(scalarOf(node))} at line ${workflow.lineOf(node)} instead of an immutable commit pin`,
    )
  }
  const repository = reference.slice(0, reference.indexOf('@'))
  if (!allowedActions.includes(repository)) {
    fail(
      `unexpected-action: ${workflow.name} uses ${repository}, which the CI policy does not allow`,
    )
  }
  if (!/^\s*v[0-9]+\.[0-9]+\.[0-9]+[0-9a-zA-Z.-]*$/.test(node.comment ?? '')) {
    fail(
      `undocumented-action-release: ${workflow.name} pins ${repository} at line ${workflow.lineOf(node)} without recording its release identity`,
    )
  }
  return repository
}

// GitHub matches a step's inputs without regard to case: the runner
// collects a `with:` mapping into one dictionary of case-insensitive
// keys, assigning each value in turn
// (PipelineTemplateConverter.ConvertToStepInputs), so
// `persist-credentials` and `PERSIST-CREDENTIALS` are not two inputs
// but one, and the action reads whichever value the runner kept last.
// The parser's uniqueness check is exact and reports no duplicate, so
// a repetition written in another case is one only this rule can see.
// Keys are judged after decoding, like every other scalar, so a casing
// that exists only once an escape is read collides like any other.
function checkCheckoutInputIdentity(workflow, inputs, line) {
  const named = new Map()
  for (const pair of inputs.items) {
    // Inputs are named by scalar keys, as entry reads them.
    if (!isScalar(pair.key)) continue
    const key = String(pair.key.value)
    const identity = key.toUpperCase()
    const first = named.get(identity)
    if (first !== undefined) {
      fail(
        `duplicate-key: in ${workflow.name}, the checkout step at line ${line} sets both '${first}' and '${key}', which name one input to the runner; the value it keeps depends on their order alone`,
      )
    }
    named.set(identity, key)
  }
}

// Every checkout must explicitly refuse to keep the workflow token on
// disk. Omission is not neutral: actions/checkout persists the token
// by default, so a step that says nothing arms every step after it.
// The one accepted value is the boolean false at exactly one path —
// the step's own `with.persist-credentials` — because that is the only
// path the action reads. A `persist-credentials` written in the step's
// env, in the job's env, or nested under another input is a different
// setting entirely and leaves the checkout at its unsafe default. The
// key is that one spelling: the inputs are held to the runner's own
// identity first, so a differently-cased sibling ends the check rather
// than being chosen between, and no other casing is the spelling this
// allow-list holds. The value is judged after decoding, so `false`,
// `False` and `FALSE` are the one boolean they all spell, while the
// string "false" is a different value and is refused.
function checkCheckoutCredentials(workflow, step, line) {
  const missing = `token-persistence: in ${workflow.name}, the checkout step at line ${line} sets no with.persist-credentials; every checkout must state persist-credentials: false`
  const inputs = entry(step, 'with')
  if (!isMap(inputs)) fail(missing)
  checkCheckoutInputIdentity(workflow, inputs, line)
  if (!hasEntry(inputs, 'persist-credentials')) fail(missing)
  const node = entry(inputs, 'persist-credentials')
  const value = scalarOf(node)
  if (value !== false) {
    fail(
      `token-persistence: in ${workflow.name}, the checkout step at line ${line} sets with.persist-credentials to ${JSON.stringify(value ?? null)}; every checkout must state persist-credentials: false`,
    )
  }
}

// Run commands stay single-line and in a form the matrix audit can
// lift: scripts/check_ci_matrix.sh extracts them from the workflow
// text and re-runs them verbatim from a fresh checkout, so a command
// the parser sees but that extraction does not — a folded scalar, a
// flow-style step, a quoted command — would be gated locally and never
// audited. The parse is the truth and the extraction is held to it.
function checkRunExtraction(workflow, commands) {
  for (const command of commands) {
    if (command.includes('\n')) {
      fail(`unextractable-run-step: ${workflow.name} has a multi-line run command`)
    }
  }
  const extracted = workflow.text
    .split('\n')
    .filter((line) => matrixRunExtraction.test(line))
    .map((line) => line.replace(matrixRunExtraction, ''))
  for (let index = 0; index < Math.max(extracted.length, commands.length); index += 1) {
    if (extracted[index] === commands[index]) continue
    fail(
      `unextractable-run-step: ${workflow.name} runs '${commands[index] ?? '(nothing)'}' where scripts/check_ci_matrix.sh would lift '${extracted[index] ?? '(nothing)'}'; the audit must re-run the commands the parser reads`,
    )
  }
}

// Installs are locked, dependencies are never mutated, and nothing
// publishes: artifacts and releases belong to later tickets. A publish
// command is diagnosed as publication rather than as a dependency
// mutation, so the rule that names it is the one that judges it.
function checkRunCommands(workflow, commands) {
  checkRunExtraction(workflow, commands)
  let foundGates = false
  for (const command of commands) {
    if (command.startsWith('pnpm install') && !command.includes('--frozen-lockfile')) {
      fail(`unlocked-install: ${workflow.name} runs '${command}' without --frozen-lockfile`)
    }
    if (command.startsWith('cargo fetch') && !command.includes('--locked')) {
      fail(`unlocked-install: ${workflow.name} runs '${command}' without --locked`)
    }
    if (
      /^\s*(pnpm|npm|yarn)\s+(add|remove|update|install|ci)(\s|$)/.test(command) &&
      command !== 'pnpm install --frozen-lockfile'
    ) {
      fail(
        `dependency-mutation: ${workflow.name} runs '${command}', which installs or mutates dependencies outside the locked path`,
      )
    }
    if (/(gh\s+(release|api)|cargo\s+publish|(pnpm|npm|yarn)\s+publish)/.test(command)) {
      fail(`implicit-publication: ${workflow.name} runs '${command}'`)
    }
    if (command === 'just check') foundGates = true
  }
  if (!foundGates) {
    fail(`missing-repository-gates: ${workflow.name} never runs 'just check', the repository-owned gate`)
  }
}

// Walk the jobs and their steps as the structures they are. A shape
// the policy cannot walk is refused rather than skipped, so no step
// can hide from the rules by being written somewhere they do not look.
function checkJobs(workflow) {
  const jobs = entry(workflow.doc.contents, 'jobs')
  if (!isMap(jobs) || jobs.items.length === 0) {
    fail(`malformed-workflow: ${workflow.name} declares no jobs mapping`)
  }
  const commands = []
  for (const jobPair of jobs.items) {
    const id = stringOf(jobPair.key) ?? '?'
    const job = jobPair.value
    if (!isMap(job)) fail(`malformed-workflow: ${workflow.name} job ${id} is not a mapping`)
    if (hasEntry(job, 'permissions')) {
      checkPermissionsBlock(workflow, entry(job, 'permissions'), `job ${id}`)
    }
    const steps = entry(job, 'steps')
    if (!isSeq(steps) || steps.items.length === 0) {
      fail(`malformed-workflow: ${workflow.name} job ${id} declares no steps sequence`)
    }
    for (const step of steps.items) {
      if (!isMap(step)) {
        fail(`malformed-workflow: ${workflow.name} job ${id} holds a step that is not a mapping`)
      }
      const line = workflow.lineOf(step)
      const uses = entry(step, 'uses')
      const run = entry(step, 'run')
      if (uses === undefined && run === undefined) {
        fail(
          `malformed-workflow: ${workflow.name} job ${id} holds a step at line ${line} that neither uses an action nor runs a command`,
        )
      }
      if (uses !== undefined) {
        if (checkActionReference(workflow, uses) === 'actions/checkout') {
          checkCheckoutCredentials(workflow, step, line)
        }
      }
      if (run !== undefined) {
        const command = stringOf(run)
        if (command === undefined) {
          fail(`malformed-workflow: ${workflow.name} job ${id} holds a run step at line ${line} that is not a command string`)
        }
        commands.push(command)
      }
    }
  }
  checkRunCommands(workflow, commands)
}

function checkWorkflowFile(file) {
  const workflow = readWorkflow(file)
  checkExpressions(workflow)
  checkForbiddenNames(workflow)
  checkTriggers(workflow)
  checkPermissions(workflow)
  checkJobs(workflow)
}

// ---------------------------------------------------------------------
// Probes: every rule above is proven to bite on a mutated copy of the
// real workflow, and every accepted equivalent is proven to pass.
// ---------------------------------------------------------------------

let work

function fixture(name, text) {
  const path = join(work, name)
  writeFileSync(path, text)
  return path
}

// Run the policy over a fixture that must be rejected, and prove the
// rejection named the rule it enforced.
function expectRejection(marker, subject, path) {
  const result = spawnSync(process.execPath, [self, path], { encoding: 'utf8' })
  const output = `${result.stdout}${result.stderr}`
  if (result.status === 0) fail(`${subject} was accepted:\n${output}`)
  if (!output.includes(marker)) fail(`${subject} failed without "${marker}":\n${output}`)
}

// Run the policy over a fixture that must be accepted, so the
// fail-closed forms cannot over-reject allowed equivalents.
function expectAcceptance(subject, path) {
  const result = spawnSync(process.execPath, [self, path], { encoding: 'utf8' })
  if (result.status !== 0) fail(`${subject} was rejected:\n${result.stdout}${result.stderr}`)
}

// Insert lines right after the first line matching the pattern, so
// probes add realistic YAML to the workflow they mutate.
function insertAfterFirst(text, pattern, addition) {
  const lines = text.split('\n')
  const index = lines.findIndex((line) => pattern.test(line))
  if (index < 0) fail(`probe-source: no line matches ${pattern}; the probes stand in for a shape that moved`)
  lines.splice(index + 1, 0, ...addition.split('\n'))
  return lines.join('\n')
}

// Substitute a pattern that must appear exactly once, so a probe can
// never silently rewrite nothing or rewrite more than it stands for.
function substituteOnce(text, pattern, replacement) {
  const matches = text.match(new RegExp(pattern.source, `${pattern.flags.replace('g', '')}g`))
  if (!matches || matches.length !== 1) {
    fail(`probe-source: ${pattern} matches ${matches ? matches.length : 0} times; the probes stand in for exactly one`)
  }
  return text.replace(pattern, replacement)
}

// Add an environment block to the gates step, the probes' realistic
// home for a value the workflow would evaluate.
function withProbeEnv(text, ...lines) {
  return insertAfterFirst(text, /^      - name: Run the repository gates$/, ['        env:', ...lines].join('\n'))
}

// Add steps after the last one, so a probe's action or command joins a
// real job rather than a fabricated one.
function withExtraSteps(text, ...lines) {
  return insertAfterFirst(text, /^        run: just check$/, lines.join('\n'))
}

// Rewrite the checkout step's persist-credentials line as the given
// replacement lines, so probes can spell the setting every way the
// policy must judge. The line must be there exactly once, or a future
// workflow could move it and leave the probes silently editing
// nothing.
function replaceCheckoutCredentials(text, replacement) {
  const lines = text.split('\n')
  const found = lines.reduce((all, line, index) => (/^\s*persist-credentials:/.test(line) ? [...all, index] : all), [])
  if (found.length !== 1) {
    fail(`probe-source: the workflow states persist-credentials ${found.length} times; the probes stand in for exactly one`)
  }
  lines.splice(found[0], 1, ...replacement.split('\n'))
  return lines.join('\n')
}

// Rewrite the whole `with:` block of the checkout step — its key line,
// the comments that document the refusal, and the refusal itself — so
// probes can spell the step's inputs in flow style as well as block
// style. The block is found from the refusal outwards, so it is the
// checkout's own inputs rather than another step's.
function replaceCheckoutWith(text, replacement) {
  const lines = text.split('\n')
  const credentials = lines.findIndex((line) => /^\s*persist-credentials:/.test(line))
  if (credentials < 0) fail('probe-source: the workflow states no persist-credentials; the probes stand in for one')
  let opening = credentials
  while (opening >= 0 && !/^\s*with:\s*$/.test(lines[opening])) {
    if (opening !== credentials && !/^\s*#/.test(lines[opening])) {
      fail(`probe-source: the checkout inputs hold '${lines[opening]}'; the probes stand in for a comment-only block`)
    }
    opening -= 1
  }
  if (opening < 0) fail('probe-source: the persist-credentials refusal is not inside a with block')
  lines.splice(opening, credentials - opening + 1, ...replacement.split('\n'))
  return lines.join('\n')
}

// Rewrite the job-level permissions block as the given replacement, so
// probes can spell the same mapping in every YAML form the policy must
// judge alike. The block must be the two lines the probes stand in
// for, or a future scope would leave them editing the wrong lines
// without failing.
function replaceJobPermissions(text, replacement) {
  const lines = text.split('\n')
  const opening = lines.findIndex((line) => /^    permissions:$/.test(line))
  if (opening < 0) fail('probe-source: the workflow has no job permissions block')
  if (!/^      [A-Za-z-]+: (read|none)$/.test(lines[opening + 1] ?? '')) {
    fail(`probe-source: the job permissions block holds '${lines[opening + 1]}'; the probes stand in for exactly one read or none scope`)
  }
  if (/^      [A-Za-z-]+:/.test(lines[opening + 2] ?? '')) {
    fail('probe-source: the job permissions block holds more than one scope; the probes stand in for exactly one')
  }
  lines.splice(opening, 2, ...replacement.split('\n'))
  return lines.join('\n')
}

function provePolicyProbes(source) {
  const text = readFileSync(source, 'utf8')

  // A construct the policy cannot judge ends the check. These prove
  // the parser is asked about each one rather than trusted to have
  // flattened it: a duplicate key, an anchor, a merge, an explicit
  // tag, a second document, a schema directive, and a plain scalar
  // YAML 1.1 and YAML 1.2 read differently.
  expectRejection('duplicate-key', 'a repeated top-level permissions block',
    fixture('duplicate-permissions.yml', insertAfterFirst(text, /^permissions:$/, '  contents: read\npermissions:')))

  expectRejection('duplicate-key', 'a repeated persist-credentials key',
    fixture('repeated-credentials.yml',
      replaceCheckoutCredentials(text, '          persist-credentials: false\n          persist-credentials: true')))

  // A merge needs an anchor, and the anchor is refused first, so a
  // merge cannot be written into a workflow this policy accepts.
  expectRejection('unsupported-construct', 'an anchored mapping reused by an alias',
    fixture('alias.yml', withExtraSteps(text,
      '      - name: Anchor an environment',
      "        env: &probe",
      "          SAFE: 'x'",
      '        run: just check',
      '      - name: Reuse the anchored environment',
      '        env: *probe',
      '        run: just check')))

  expectRejection('unsupported-construct', 'a merge key, which needs the anchor the policy refuses first',
    fixture('merge.yml', withExtraSteps(text,
      '      - name: Anchor an environment',
      "        env: &probe",
      "          SAFE: 'x'",
      '        run: just check',
      '      - name: Merge the anchored environment',
      '        env:',
      '          <<: *probe',
      '        run: just check')))

  expectRejection('unsupported-construct', 'an explicitly tagged scalar',
    fixture('explicit-tag.yml', substituteOnce(text, /^          node-version: 24$/m, '          node-version: !!str "24"')))

  expectRejection('unsupported-construct', 'a second document in the stream',
    fixture('multi-document.yml', `${text}---\nname: Second\n`))

  expectRejection('unsupported-construct', 'a %YAML schema directive',
    fixture('yaml-directive.yml', `%YAML 1.1\n---\n${text}`))

  expectRejection('ambiguous-schema-value', 'an unquoted YAML 1.1 boolean',
    fixture('ambiguous-boolean.yml', withProbeEnv(text, '          PROBE: yes')))

  expectRejection('ambiguous-schema-value', 'an unquoted leading-zero integer',
    fixture('ambiguous-octal.yml', withProbeEnv(text, '          PROBE: 0777')))

  expectRejection('unreadable-workflow', 'a workflow that does not parse',
    fixture('unreadable.yml', substituteOnce(text, /^name: CI$/m, 'name: [CI')))

  // Mutable action reference: a version tag instead of a commit.
  expectRejection('mutable-action-reference', 'a version-tagged action reference',
    fixture('mutable-action.yml', substituteOnce(text, /(pnpm\/action-setup)@[0-9a-f]{40}/, '$1@v6.1.0')))

  // Undocumented pin: the release identity comment is dropped.
  expectRejection('undocumented-action-release', 'an action pin without its release identity',
    fixture('undocumented-action.yml',
      substituteOnce(text, /(actions\/checkout@[0-9a-f]{40})[ \t]*#[ \t]*v[0-9.]+/, '$1')))

  // An allowlisted shape is not enough: upload-artifact is publication.
  expectRejection('unexpected-action', 'an action outside the allowlist',
    fixture('unexpected-action.yml', withExtraSteps(text,
      '      - uses: actions/upload-artifact@0123456789abcdef0123456789abcdef01234567 # v0.0.0')))

  // The action rules only bite on steps the policy reads, so a step
  // written as a bare sequence item, in flow style, or under a quoted
  // key must be read too. An unpinned action written the first way
  // once passed unseen.
  for (const [label, step] of [
    ['a bare sequence item', '      - uses: unpinned/action@main'],
    ['flow style', '      - {uses: unpinned/action@main}'],
    ['a quoted key', '      - "uses": unpinned/action@main'],
  ]) {
    expectRejection('mutable-action-reference', `an unpinned action written as ${label}`,
      fixture(`uses-${label.replace(/\W+/g, '-')}.yml`, withExtraSteps(text, step)))
  }

  // Privileged event: pull_request_target hands untrusted code the
  // repository's own authority.
  expectRejection('privileged-event', 'a pull_request_target trigger',
    fixture('privileged-event.yml', substituteOnce(text, /^  pull_request:$/m, '  pull_request_target:')))

  // Unexpected trigger: manual dispatch is a trigger the policy never
  // allowed, and a workflow that stops running on pull requests stops
  // gating the code the policy is about.
  expectRejection('unexpected-trigger', 'a manual dispatch trigger',
    fixture('unexpected-trigger.yml', insertAfterFirst(text, /^  pull_request:$/, '  workflow_dispatch:')))

  expectRejection('unexpected-trigger', 'a workflow that no longer runs on pull requests',
    fixture('missing-pull-request.yml', substituteOnce(text, /^  pull_request:\n/m, '')))

  // Unexpected push refs: another branch, and the release tags that
  // belong to a release ticket rather than to this gate.
  expectRejection('unexpected-push-refs', 'pushes gating a branch other than main',
    fixture('unexpected-push-refs.yml', substituteOnce(text, /^      - main$/m, '      - release')))

  expectRejection('unexpected-push-refs', 'pushes gating release tags',
    fixture('tag-push-refs.yml', insertAfterFirst(text, /^      - main$/, '    tags:\n      - "v*"')))

  // Omitting persist-credentials is not neutral: actions/checkout keeps
  // the workflow token on disk by default, so a checkout that says
  // nothing arms every step after it. Only the boolean false at the
  // step's own with.persist-credentials is accepted, so an absent
  // value, a decoy at another path, a wrong type, an expression, and a
  // true must each fail rather than be interpreted.
  const withoutRefusal = replaceCheckoutCredentials(text, '          fetch-depth: 0')

  expectRejection('token-persistence', 'a checkout that never sets persist-credentials',
    fixture('default-credentials.yml', withoutRefusal))

  expectRejection('token-persistence', "a decoy refusal in the checkout step's env",
    fixture('env-decoy-credentials.yml',
      insertAfterFirst(withoutRefusal, /^          fetch-depth: 0$/, '        env:\n          persist-credentials: false')))

  expectRejection('token-persistence', "a decoy refusal in the job's env",
    fixture('job-env-decoy-credentials.yml',
      insertAfterFirst(withoutRefusal, /^    runs-on: macos-latest$/, '    env:\n      persist-credentials: false')))

  expectRejection('token-persistence', 'a decoy refusal nested under another checkout input',
    fixture('nested-decoy-credentials.yml',
      replaceCheckoutCredentials(text, '          fetch-depth: 0\n          submodules:\n            persist-credentials: false')))

  expectRejection('token-persistence', 'a decoy refusal nested in a list under another checkout input',
    fixture('listed-decoy-credentials.yml',
      replaceCheckoutCredentials(text, '          sparse-checkout:\n            - persist-credentials: false')))

  expectRejection('token-persistence', 'a checkout that keeps the workflow token',
    fixture('persisted-credentials.yml', replaceCheckoutCredentials(text, '          persist-credentials: true')))

  expectRejection('token-persistence', 'a quoted persist-credentials value',
    fixture('quoted-credentials.yml', replaceCheckoutCredentials(text, '          persist-credentials: "false"')))

  expectRejection('token-persistence', 'a persist-credentials value left to an expression',
    fixture('expression-credentials.yml',
      replaceCheckoutCredentials(text, '          persist-credentials: ${{ github.ref }}')))

  // One input, two spellings: GitHub matches a step's inputs without
  // regard to case, so a sibling key that differs only in case is not
  // a second input but the same one written twice, and the action
  // reads whichever value the runner kept. The parser's uniqueness
  // check is exact and reports no duplicate, so each of these arrives
  // at the policy as a valid refusal standing beside the key that
  // would overrule it.
  // The escaped sibling spells its casing with a Unicode escape, so
  // the two keys only collide once the parser has decoded them; the
  // fragment is not prose. cspell:ignore credentia
  for (const [label, inputs] of [
    ['after the refusal', '          persist-credentials: false\n          PERSIST-CREDENTIALS: true'],
    ['before the refusal', '          PERSIST-CREDENTIALS: true\n          persist-credentials: false'],
    ['in mixed case', '          persist-credentials: false\n          Persist-Credentials: true'],
    ['spelled by a Unicode escape', '          persist-credentials: false\n          "PERSIST-CREDENTIA\\u004cS": true'],
    ['refusing again', '          persist-credentials: false\n          PERSIST-CREDENTIALS: false'],
    ['written as a string', '          persist-credentials: false\n          PERSIST-CREDENTIALS: "false"'],
    ['left empty', '          persist-credentials: false\n          PERSIST-CREDENTIALS:'],
    ['left to an expression', '          persist-credentials: false\n          PERSIST-CREDENTIALS: ${{ github.ref }}'],
  ]) {
    expectRejection('duplicate-key', `a persist-credentials sibling ${label}`,
      fixture(`sibling-credentials-${label.replace(/\W+/g, '-')}.yml`, replaceCheckoutCredentials(text, inputs)))
  }

  // The rule is per checkout, not per workflow: a later checkout cannot
  // ride on the first one's refusal.
  expectRejection('token-persistence', 'a second checkout that refuses nothing',
    fixture('second-checkout.yml', withExtraSteps(text,
      '      - name: Check out again',
      '        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1')))

  // The secret and token rules are the expression allow-list working
  // rather than a list of spellings, so every equivalent way to reach
  // the same value must fail: whole-context serialisation, a nested
  // unwrap, case-insensitive context aliases, dot syntax, index
  // syntax, interior spacing, and — because the allow-list is applied
  // to decoded scalars — the escaped and continued spellings that only
  // become expressions once the parser has read them.
  // The escaped probes below split a context name mid-word to prove the
  // allow-list judges the decoded spelling; the fragment is not prose.
  // cspell:ignore ithub
  const adversarial = [
    ['secret-exposure', 'a whole-context secret serialisation', '${{ toJSON(secrets) }}'],
    ['secret-exposure', 'a nested secret unwrap', '${{ fromJSON(toJSON(secrets)).APPLE_SIGNING_KEY }}'],
    ['secret-exposure', 'an upper-case secrets alias', '${{ SECRETS.APPLE_SIGNING_KEY }}'],
    ['secret-exposure', 'a mixed-case secrets alias', '${{ Secrets.Apple_Signing_Key }}'],
    ['secret-exposure', 'a dot-syntax secret reference', '${{ secrets.APPLE_SIGNING_KEY }}'],
    ['secret-exposure', 'a spaced secrets property reference', '${{ secrets . APPLE_SIGNING_KEY }}'],
    ['secret-exposure', 'a single-quoted index-syntax secret reference', "${{ secrets['APPLE_SIGNING_KEY'] }}"],
    ['secret-exposure', 'a double-quoted index-syntax secret reference', '${{ secrets[ "DEPLOY_KEY" ] }}'],
    ['secret-exposure', 'an upper-case index-syntax secret reference', "${{ SECRETS['DEPLOY_KEY'] }}"],
    ['token-exposure', 'a dot-syntax workflow token reference', '${{ github.token }}'],
    ['token-exposure', 'a mixed-case workflow token alias', '${{ GitHub.Token }}'],
    ['token-exposure', 'a spaced workflow token reference', '${{ github . token }}'],
    ['token-exposure', 'an index-syntax workflow token reference', "${{ github['token'] }}"],
    ['token-exposure', 'an upper-case index-syntax token reference', '${{ GITHUB["TOKEN"] }}'],
    ['unsafe-expression', 'a whole-context github serialisation', '${{ toJSON(github) }}'],
    ['unsafe-expression', 'a nested workflow token unwrap', '${{ fromJSON(toJSON(github)).token }}'],
    ['unsafe-expression', 'an untrusted pull-request title', '${{ github.event.pull_request.title }}'],
    // Only the parser makes these expressions at all: the escape and
    // the continuation are decoded before the allow-list sees them, so
    // an expression whose delimiters or context name exist only after
    // decoding is invisible to any scan of the lines it was written on.
    ['token-exposure', 'a Unicode-escaped workflow token context', '"${{ \\u0067ithub.token }}"'],
    ['secret-exposure', 'a Unicode-escaped secrets context', '"${{ toJSON(sec\\u0072ets) }}"'],
    ['token-exposure', 'a Unicode-escaped delimiter around the workflow token', '"\\u0024{{ github.token }}"'],
    ['secret-exposure', 'a Unicode-escaped delimiter around the secrets context', '"\\u0024{{ toJSON(secrets) }}"'],
    ['token-exposure', 'a workflow token reference continued across lines', '"${{ github.\\\n            token }}"'],
    ['secret-exposure', 'a secret serialisation continued across lines', '"${{ toJSON(sec\\\n            rets) }}"'],
    ['token-exposure', 'a delimiter continued across lines around the workflow token', '"$\\\n            {{ github.token }}"'],
    ['secret-exposure', 'a delimiter continued across lines around the secrets context', '"$\\\n            {{ toJSON(secrets) }}"'],
  ]
  adversarial.forEach(([marker, subject, expression], index) => {
    expectRejection(marker, subject, fixture(`expression-${index + 1}.yml`, withProbeEnv(text, `          PROBE: ${expression}`)))
  })

  // An expression that does not close is refused rather than parsed,
  // so nothing can hide in the part the scan never reads.
  expectRejection('unterminated-expression', 'an expression that never closes',
    fixture('unterminated-expression.yml', withProbeEnv(text, '          PROBE: ${{ secrets.APPLE_SIGNING_KEY')))

  // A conditional is an expression GitHub evaluates with or without
  // ${{ }}, so an unwrapped one reaches a context just as surely. A
  // condition on a secret is an oracle even when its value never
  // reaches the log.
  for (const [marker, subject, condition] of [
    ['secret-exposure', 'an unwrapped conditional on a secret', "        if: secrets.APPLE_SIGNING_KEY != ''"],
    ['secret-exposure', 'an unwrapped conditional over the whole secrets context', "        if: contains(toJSON(secrets), 'x')"],
    ['unsafe-expression', 'an unwrapped conditional the allow-list does not hold', "        if: github.event_name == 'push'"],
    ['unsafe-expression', 'a conditional that only partly wraps its expression', "        if: ${{ github.ref }} == 'refs/heads/main'"],
  ]) {
    expectRejection(marker, subject,
      fixture(`conditional-${subject.replace(/\W+/g, '-')}.yml`, insertAfterFirst(text, /^      - name: Install just and pre-commit$/, condition)))
  }

  // A flow-style step is read rather than refused as unspellable, so
  // the conditional inside one is judged like any other.
  expectRejection('unsafe-expression', 'a flow-style conditional',
    fixture('flow-conditional.yml', withExtraSteps(text, '      - {if: github.event_name, run: just check}')))

  // Write permission: contents: write for the gates job. Every other
  // spelling of the same mapping — flow style spaced, compact and
  // spread across lines, a quoted scope, a quoted key, and a string
  // grant — is the same grant and fails the same way.
  expectRejection('write-permission', 'a contents: write permission',
    fixture('write-permission.yml', text.replace(/contents: read/g, 'contents: write')))

  for (const [label, block] of [
    ['a flow-style write permission', '    permissions: { contents: write }'],
    ['a compact flow-style write permission', '    permissions: {id-token: write}'],
    ['a flow-style write permission spread across lines', '    permissions: {\n      id-token: write\n    }'],
    ['a write permission under a quoted scope', "    permissions:\n      'contents': write"],
    ['a flow-style write permission under a quoted key', '    "permissions": { contents: write }'],
    ['a string-form permissions grant', '    permissions: read-all'],
  ]) {
    expectRejection('write-permission', label,
      fixture(`permissions-${label.replace(/\W+/g, '-')}.yml`, replaceJobPermissions(text, block)))
  }

  // A blank line inside the block must not end the scan before a
  // write scope.
  expectRejection('write-permission', 'a write permission after a blank line',
    fixture('blank-line-permissions.yml', insertAfterFirst(text, /^  contents: read$/, '\n  statuses: write')))

  // Unextractable run step: a command scripts/check_ci_matrix.sh
  // cannot lift out and re-run verbatim — folded, in flow style, or
  // quoted so the audit would run a different string.
  expectRejection('unextractable-run-step', 'a multi-line run command',
    fixture('multiline-run.yml',
      substituteOnce(text, /^        run: just check$/m, '        run: |\n          just check')))

  expectRejection('unextractable-run-step', 'a folded run command written as a sequence item',
    fixture('sequence-multiline-run.yml', withExtraSteps(text, '      - run: |\n          gh release create v0 ./dist/App.dmg')))

  expectRejection('unextractable-run-step', 'a flow-style run step',
    fixture('flow-run.yml', withExtraSteps(text, '      - {run: gh release create v0 ./dist/App.dmg}')))

  expectRejection('unextractable-run-step', 'a quoted run command',
    fixture('quoted-run.yml', substituteOnce(text, /^        run: just check$/m, '        run: "just check"')))

  // Unlocked installs and dependency mutation.
  expectRejection('unlocked-install', 'an unlocked pnpm install',
    fixture('unlocked-install.yml', substituteOnce(text, /pnpm install --frozen-lockfile/, 'pnpm install')))

  expectRejection('unlocked-install', 'an unlocked cargo fetch',
    fixture('unlocked-cargo.yml', substituteOnce(text, /cargo fetch --locked/, 'cargo fetch')))

  expectRejection('dependency-mutation', 'a dependency-mutating install',
    fixture('dependency-mutation.yml', withExtraSteps(text, '      - run: pnpm add left-pad')))

  // Implicit publication, including a command written as a bare
  // sequence item — a shape that once hid a whole command from every
  // rule below it and from the CI matrix audit.
  expectRejection('implicit-publication', 'a release-publication command',
    fixture('implicit-publication.yml', withExtraSteps(text, '      - run: gh release create v0 ./dist/App.dmg')))

  expectRejection('implicit-publication', 'a package-publication command',
    fixture('package-publication.yml', withExtraSteps(text, '      - run: pnpm publish --access public')))

  // Planning artifact input: the local PM gate is not a CI input.
  expectRejection('planning-artifact-input', 'a temp/project-management input',
    fixture('planning-input.yml', withExtraSteps(text, '      - run: temp/project-management/check_planning.sh')))

  // Missing repository gates: building is not gating.
  expectRejection('missing-repository-gates', 'a workflow without just check',
    fixture('missing-gates.yml', substituteOnce(text, /^        run: just check$/m, '        run: just build')))

  // Positive probes: the fail-closed forms must not over-reject the
  // allowed equivalents. Normalisation and decoding have to work in
  // both directions to be a normal form rather than a filter, and a
  // rule that judges the parsed document must accept every spelling
  // that parses to the safe value.
  expectAcceptance('an extended read-only permissions block',
    fixture('allowed-permissions.yml', replaceJobPermissions(text,
      '    permissions:\n      contents: read\n      issues: none\n      # read-only for untrusted pull-request code')))

  expectAcceptance('a flow-style read-only permissions block',
    fixture('allowed-flow-permissions.yml', replaceJobPermissions(text, '    permissions: { contents: read }')))

  expectAcceptance('index syntax on an allowed expression',
    fixture('allowed-bracket-expression.yml', substituteOnce(text, /github\.ref/, "github['ref']")))

  expectAcceptance('spaced double-quoted index syntax on an allowed expression',
    fixture('allowed-quoted-expression.yml', substituteOnce(text, /github\.ref/, 'github[ "ref" ]')))

  expectAcceptance('a mixed-case alias of an allowed expression',
    fixture('allowed-mixed-case-expression.yml', substituteOnce(text, /github\.ref/, 'GitHub.Ref')))

  expectAcceptance('a Unicode-escaped spelling of an allowed expression',
    fixture('allowed-escaped-expression.yml',
      substituteOnce(text, /^  group: ci-\$\{\{ github\.ref \}\}$/m, '  group: "ci-${{ \\u0067ithub.ref }}"')))

  expectAcceptance('a quoted value that would otherwise be schema-ambiguous',
    fixture('allowed-quoted-ambiguous.yml', withProbeEnv(text, '          PROBE: "yes"')))

  expectAcceptance('a credential refusal beside other checkout inputs',
    fixture('allowed-checkout-inputs.yml',
      replaceCheckoutCredentials(text, '          fetch-depth: 1\n          persist-credentials: false # no token on disk')))

  expectAcceptance('a credential refusal after a nested list input',
    fixture('allowed-nested-checkout-input.yml',
      replaceCheckoutCredentials(text, '          sparse-checkout:\n            - src\n          persist-credentials: false')))

  // The parser decodes false, False and FALSE to the one boolean they
  // all spell, so all three are the accepted refusal; the string
  // "false" above is a different value and is not.
  expectAcceptance('an upper-case spelling of the credential refusal',
    fixture('allowed-capitalised-credentials.yml', replaceCheckoutCredentials(text, '          persist-credentials: FALSE')))

  expectAcceptance('a flow-style credential refusal',
    fixture('allowed-flow-credentials.yml', replaceCheckoutWith(text, '        with: {persist-credentials: false}')))
}

const requested = process.argv.slice(2)
if (requested.length > 0) {
  for (const file of requested) checkWorkflowFile(file)
  process.exit(0)
}

let workflows = []
try {
  workflows = readdirSync('.github/workflows')
    .filter((entry) => entry.endsWith('.yml') || entry.endsWith('.yaml'))
    .sort()
    .map((entry) => `.github/workflows/${entry}`)
} catch {
  workflows = []
}
if (workflows.length === 0) fail('no-workflows: nothing is committed under .github/workflows')
for (const file of workflows) checkWorkflowFile(file)

work = mkdtempSync(join(tmpdir(), 'kanban-workflow-policy-'))
process.on('exit', () => rmSync(work, { recursive: true, force: true }))
provePolicyProbes(workflows[0])
process.stdout.write(`check-workflow-policy: ${workflows.length} workflow(s) hold the least-privilege policy\n`)
