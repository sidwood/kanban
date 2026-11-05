"""The eleven operator MCP acceptance tests, one check per behaviour.

Tests 1-10 are the scenarios in docs/mcp-acceptance-tests.md. Test 11
covers sub-issues as docs/sub-issues.md and the Linear issue model
describe them. Each check asserts what Linear does, not merely that a
call succeeded. A waiver must name the exact failure it still expects;
any other failure fails the run.
"""

from pathlib import Path

from .context import (
    CLOSED_TYPES,
    COLOUR_PATTERN,
    DATE_PATTERN,
    KEY_PATTERN,
    STATE_TYPES,
    StepError,
    changed_fields,
    has_relation,
    key_number,
    parse_timestamp,
    relation_set,
)

FIXTURES = Path(__file__).resolve().parent / "fixtures"

OPERATOR_TOOLS = {
    "save_issue", "get_issue", "list_issues", "list_issue_statuses", "list_issue_labels",
    "list_milestones", "list_projects", "create_milestone", "create_label", "retire_label",
    "update_project_settings", "comment_create", "comment_edit", "comment_revisions",
    "list_comments", "resolve_comment",
}

LEGACY_PRIORITY = {"urgent": 1, "high": 2, "normal": 3, "low": 4}

REQUIRED_FIELDS = {
    "identifier": str, "url": str, "ticket_id": int, "project_id": int, "title": str,
    "description": str, "state": str, "state_type": str, "priority": int, "labels": list,
    "relations": list, "state_history": list, "version": int, "created_at": str,
    "updated_at": str,
}
OPTIONAL_FIELDS = {
    "due_date": str, "milestone_id": int, "started_at": str, "completed_at": str,
    "canceled_at": str, "parent": str, "sub_issues": dict, "duplicate_of": str,
    "duplicates": list, "children": list, "assignee": str,
}


# Setup ---------------------------------------------------------------------


def register_project(ctx, code, name, repository):
    """Register one project over the socket. No MCP tool does this."""
    payload = {
        "mutation": {"optimistic_version": 0, "idempotency_key": ctx.idempotency_key("register")},
        "code": code,
        "name": name,
        "repository": str(repository),
        "seed_workspace": str(repository) + ".seed",
        "default_branch": "main",
        "herdr_workspace": code.lower(),
        "herdr_session": code.lower() + "-main",
    }
    try:
        return ctx.socket.command("project.register", payload)
    except Exception as error:  # a stale version names the one to use
        current = getattr(error, "error", {}).get("current_version")
        if current is None:
            raise StepError(f"project.register {code} was refused: {error}")
        payload["mutation"] = {"optimistic_version": current,
                               "idempotency_key": ctx.idempotency_key("register")}
        return ctx.socket.command("project.register", payload)


def setup_fresh(ctx, repositories):
    """Register LIN and LTB and create the milestones and labels."""
    rec = ctx.rec
    rec.begin(0, "Setup")
    ctx.project_code, ctx.other_code = "LIN", "LTB"
    for code, name in (("LIN", "Linear parity"), ("LTB", "Linear parity B")):
        record = register_project(ctx, code, name, repositories[code])
        ctx.projects[code] = record["id"]
        rec.equal(f"project.register {code} returns the code", record.get("code"), code)
    tools = set(ctx.mcp.list_tools())
    rec.check("tools/list offers every operator tool", OPERATOR_TOOLS <= tools,
              "missing: " + ", ".join(sorted(OPERATOR_TOOLS - tools)) if OPERATOR_TOOLS - tools
              else f"{len(tools)} tools")
    for label, project, name, target in (
        ("M1", "LIN", "M1 Staging", "2030-03-31"),
        ("M2", "LIN", "M2 Launch", "2030-06-30"),
        ("B1", "LTB", "B1 Other team milestone", None),
    ):
        arguments = {"mutation": ctx.mutation(0, "milestone"), "project_id": ctx.projects[project],
                     "name": name}
        if target:
            arguments["target_date"] = target
        ctx.milestones[label] = ctx.must("create_milestone", arguments, f"create_milestone {name}")["id"]
    labels = [
        ("LIN", "frontend", "#1F77B4", None, False),
        ("LIN", "storage", "#FF7F0E", None, False),
        ("LIN", "platform", "#2CA02C", None, False),
        ("LIN", "needs-review", "#D62728", None, False),
        ("LIN", "size-small", "#9467BD", "Size", True),
        ("LIN", "size-medium", "#8C564B", "Size", True),
        ("LIN", "size-large", "#E377C2", "Size", True),
        ("LIN", "docs", "#7F7F7F", "Topic", False),
        ("LIN", "testing", "#BCBD22", "Topic", False),
        ("LIN", "legacy-tag", "#17BECF", None, False),
        ("LTB", "frontend", "#1F77B4", None, False),
    ]
    for project, name, colour, group, single in labels:
        arguments = {"mutation": ctx.mutation(0, "label"), "project_id": ctx.projects[project],
                     "name": name, "color": colour}
        if group:
            arguments["group"] = group
            arguments["group_single_select"] = single
        ctx.must("create_label", arguments, f"create_label {name}")
    # A label retired while an issue holds it: Linear keeps it there.
    ctx.retired_holder = ctx.create("Setup holder of a retired label", "LIN", add_labels=["legacy-tag"])
    retired = ctx.must("retire_label", {"mutation": ctx.mutation(0, "retire"),
                                        "project_id": ctx.project_id, "name": "legacy-tag"})
    rec.equal("retire_label returns the retired record", retired.get("retired"), True)
    rec.check("setup created milestones and labels", True,
              f"milestones {sorted(ctx.milestones)}; {len(labels)} labels")


# Shared record checks ------------------------------------------------------


def record_problems(record):
    """Why one issue record is incomplete or mistyped, if it is."""
    problems = []
    for name, kind in REQUIRED_FIELDS.items():
        if name not in record:
            problems.append(f"missing {name}")
        elif not isinstance(record[name], kind) or (kind is int and isinstance(record[name], bool)):
            problems.append(f"{name} is {type(record[name]).__name__}")
    for name, kind in OPTIONAL_FIELDS.items():
        if name in record and not isinstance(record[name], kind):
            problems.append(f"{name} is {type(record[name]).__name__}")
    unknown = set(record) - set(REQUIRED_FIELDS) - set(OPTIONAL_FIELDS)
    if unknown:
        problems.append("unexpected " + ", ".join(sorted(unknown)))
    if problems:
        return problems
    if not KEY_PATTERN.match(record["identifier"]):
        problems.append(f"identifier {record['identifier']!r} is not TEAM-N")
    if not record["url"].endswith(record["identifier"]):
        problems.append("url does not name the key")
    if not 0 <= record["priority"] <= 4:
        problems.append(f"priority {record['priority']} outside 0-4")
    if record["state_type"] not in STATE_TYPES:
        problems.append(f"state_type {record['state_type']!r}")
    for name in ("created_at", "updated_at", "started_at", "completed_at", "canceled_at"):
        if name in record and parse_timestamp(record[name]) is None:
            problems.append(f"{name} {record[name]!r} is not RFC 3339")
    if "due_date" in record and not DATE_PATTERN.match(record["due_date"]):
        problems.append(f"due_date {record['due_date']!r} is not YYYY-MM-DD")
    created, updated = parse_timestamp(record["created_at"]), parse_timestamp(record["updated_at"])
    if created and updated and updated < created:
        problems.append("updated_at is before created_at")
    if record["labels"] != sorted(set(record["labels"]), key=record["labels"].index):
        problems.append("labels repeat")
    return problems


def history_problems(record):
    """Why an issue's state history is not a contiguous Linear audit."""
    rows = record.get("state_history", [])
    if not rows:
        return ["no state history rows"]
    problems = []
    starts = [parse_timestamp(row.get("started_at")) for row in rows]
    if None in starts:
        return ["a history row has no RFC 3339 started_at"]
    if starts != sorted(starts):
        problems.append("rows are not in time order")
    for index, row in enumerate(rows[:-1]):
        if row.get("ended_at") is None:
            problems.append(f"row {index + 1} ({row['state']}) never ended")
        elif row["ended_at"] != rows[index + 1]["started_at"]:
            problems.append(f"row {index + 1} ends at {row['ended_at']} but row {index + 2} "
                            f"starts at {rows[index + 1]['started_at']}")
    last = rows[-1]
    if last.get("ended_at") is not None:
        problems.append("the current row has an ended_at")
    if (last.get("state"), last.get("state_type")) != (record.get("state"), record.get("state_type")):
        problems.append(f"last row is {last.get('state')} but the issue is {record.get('state')}")
    return problems


def lifecycle_problems(record):
    """Why the started/completed/canceled stamps disagree with the state type."""
    kind = record.get("state_type")
    has = {name: name in record for name in ("started_at", "completed_at", "canceled_at")}
    problems = []
    if kind == "started" and not has["started_at"]:
        problems.append("started issue has no started_at")
    if kind == "completed" and not has["completed_at"]:
        problems.append("completed issue has no completed_at")
    if kind in ("canceled", "duplicate") and not has["canceled_at"]:
        problems.append(f"{kind} issue has no canceled_at")
    if kind != "completed" and has["completed_at"]:
        problems.append(f"{kind} issue keeps completed_at")
    if kind not in ("canceled", "duplicate") and has["canceled_at"]:
        problems.append(f"{kind} issue keeps canceled_at")
    return problems


def first_offender(records, judge):
    for record in records:
        problems = judge(record)
        if problems:
            return f"{record.get('identifier')}: {'; '.join(problems)}"
    return None


def check_every(rec, description, records, judge):
    """PASS when `judge` finds nothing wrong with any record."""
    if not records:
        rec.skip(description, "no issues to judge")
        return
    offender = first_offender(records, judge)
    rec.check(description, offender is None, offender or f"{len(records)} issues")


def history_appended(before, after, state, state_type):
    """Problems with the one history row a state move must append."""
    rows_before, rows_after = before["state_history"], after["state_history"]
    problems = []
    if len(rows_after) != len(rows_before) + 1:
        problems.append(f"{len(rows_before)} rows became {len(rows_after)}")
        return problems
    new, previous = rows_after[-1], rows_after[-2]
    if (new["state"], new["state_type"]) != (state, state_type):
        problems.append(f"new row is {new['state']}/{new['state_type']}")
    if new.get("ended_at") is not None:
        problems.append("new row is already ended")
    if previous.get("ended_at") != new["started_at"]:
        problems.append(f"previous row ended {previous.get('ended_at')}, new row started {new['started_at']}")
    if rows_after[:-2] != rows_before[:-1]:
        problems.append("older rows changed")
    return problems


def ids(records):
    return {record["identifier"] for record in records}


# Test 1: Create ------------------------------------------------------------

CREATE_DESCRIPTION = (
    "## Goal\n\nShip the *staging* environment.\n\n"
    "- [ ] Provision the database\n- [x] Write the runbook\n  - [ ] Nested item\n\n"
    "```sh\njust check\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n"
    "Two trailing spaces make a hard break  \n\tA tab-indented line\n"
    "Unicode: café ✅ — “quotes” 🚀\n\n"
)


def test_create(ctx):
    rec = ctx.rec
    saved = ctx.must("save_issue", {
        "mutation": ctx.mutation(0, "create"), "project_id": ctx.project_id,
        "title": "T1 Provision staging", "description": CREATE_DESCRIPTION, "state": "Ready",
        "priority": 2, "add_labels": ["frontend", "size-small"], "milestone_id": ctx.milestones["M1"],
        "due_date": "2030-03-15",
    }, "create T1")
    key = saved.get("identifier", "")
    rec.check("save_issue returns a TEAM-N key in the team", KEY_PATTERN.match(key) is not None
              and key.startswith("LIN-"), key)
    rec.check("save_issue returns a link that names the key",
              isinstance(saved.get("url"), str) and saved["url"].endswith(key), str(saved.get("url")))
    issue = ctx.get(key)
    rec.equal("get_issue returns the same link", issue["url"], saved.get("url"))
    rec.equal("title is stored as sent", issue["title"], "T1 Provision staging")
    rec.equal("Markdown description round-trips byte for byte", issue["description"], CREATE_DESCRIPTION)
    rec.equal("state is the one sent, with its type", (issue["state"], issue["state_type"]),
              ("Ready", "unstarted"))
    rec.equal("priority is stored on Linear's 0-4 scale", issue["priority"], 2)
    rec.equal("labels are the ones sent", sorted(issue["labels"]), ["frontend", "size-small"])
    rec.equal("issue belongs to the team it was created in", issue["project_id"], ctx.project_id)
    rec.equal("milestone is the one sent", issue.get("milestone_id"), ctx.milestones["M1"])
    rec.equal("due date is a plain date, as sent", issue.get("due_date"), "2030-03-15")
    rows = issue["state_history"]
    rec.check("create writes one state history row for the initial state",
              len(rows) == 1 and rows[0]["state"] == "Ready" and rows[0].get("ended_at") is None
              and rows[0]["started_at"] == issue["created_at"],
              f"{len(rows)} rows: {[(row['state'], row['started_at']) for row in rows]}")
    rec.check("an unstarted issue carries no started, completed or canceled stamp",
              not any(name in issue for name in ("started_at", "completed_at", "canceled_at")),
              ", ".join(name for name in ("started_at", "completed_at", "canceled_at") if name in issue)
              or "none")
    problems = record_problems(issue)
    rec.check("the record has every field with its type", not problems, "; ".join(problems) or "complete")

    minimal_key = ctx.create("T1 Minimal", "LIN")
    minimal = ctx.get(minimal_key)
    rec.equal("keys follow the team's sequence", key_number(minimal_key), key_number(key) + 1)
    rec.check("a bare create defaults to no priority (0) and a backlog state",
              minimal["priority"] == 0 and minimal["state_type"] == "backlog",
              f"priority {minimal['priority']}, {minimal['state']}/{minimal['state_type']}")
    present = [name for name in ("due_date", "milestone_id", "parent", "assignee") if name in minimal]
    present += ["labels"] if minimal["labels"] else []
    present += ["description"] if minimal["description"] else []
    rec.check("a bare create carries no labels, due date, milestone or description", not present,
              "holds " + ", ".join(present) if present else "none")
    other_key = ctx.create("T1 Other team", "LTB")
    rec.check("another team keeps its own key sequence", other_key.startswith("LTB-"), other_key)
    listed = ids(ctx.list_all(project_id=ctx.project_id))
    rec.check("the new issues are listed in their team", {key, minimal_key} <= listed,
              f"{len(listed)} listed")

    # Legacy socket creation should land on the same issue model.
    numbers = {}
    for priority in ("urgent", "high", "normal", "low"):
        created = ctx.legacy("ticket.create", {
            "mutation": ctx.mutation(0, "legacy-create"), "project_id": ctx.other_project_id,
            "kind": "task", "priority": priority, "title": f"T1 Legacy {priority}",
            "subtype": "operational", "mode": "human", "completion": ["Done"],
        })
        numbers[priority] = created["number"]
    legacy = {priority: ctx.get(f"{ctx.other_code}-{number}") for priority, number in numbers.items()}
    rec.check("a legacy ticket.create is readable as an issue",
              all(record["title"] == f"T1 Legacy {priority}" for priority, record in legacy.items()),
              ", ".join(record["identifier"] for record in legacy.values()))
    rows = legacy["urgent"]["state_history"]
    rec.check("legacy ticket.create writes the initial state history row",
              len(rows) == 1 and rows[0]["state"] == legacy["urgent"]["state"],
              f"{len(rows)} rows")
    mapped = {priority: record["priority"] for priority, record in legacy.items()}
    rec.check("legacy ticket.create maps priority (urgent 1, high 2, normal 3, low 4)",
              mapped == LEGACY_PRIORITY, f"got {mapped}")


# Test 2: Read ---------------------------------------------------------------


def seed_read_corpus(ctx):
    """Issues that give every list filter a proper subset to find."""
    pl1, pl2 = ctx.milestones["M1"], ctx.milestones["M2"]
    r1 = ctx.create("T2 aurora frontend task", "LIN", state="Draft", add_labels=["frontend"],
                    milestone_id=pl1, assignee="operator")
    r2 = ctx.create("T2 zephyr database task", "LIN", state="Ready", add_labels=["storage"],
                    milestone_id=pl2)
    r3 = ctx.create("T2 aurora review", "LIN", state="In Review", add_labels=["frontend", "storage"],
                    milestone_id=pl1, assignee="operator", description="Reviewing the zephyr notes.")
    r4 = ctx.create("T2 finished work", "LIN", state="Done", add_labels=["platform"])
    r5 = ctx.create("T2 dropped work", "LIN", state="Cancelled")
    ctx.create("T2 aurora child one", parent_id=r1)
    ctx.create("T2 aurora child two", parent_id=r1, state="Done")
    ctx.save_must(r2, add_blocks=[r3])
    ctx.save_must(r4, add_related=[r5])
    ctx.save_must(r1, priority=1)
    ctx.create("T2 other team issue", "LTB")
    ctx.save_must(ctx.create("T2 zephyr duplicate", "LIN"), duplicate_of=r2)
    title_only = ctx.create("T2 marigold beacon", "LIN",
                            description="A note that does not repeat the title token.")
    description_only = ctx.create("T2 plain heading", "LIN",
                                  description="The body alone holds periwinkle.")
    ctx.create("T2 control heading", "LIN", description="A control body with neither token.")
    ctx.query_examples = {"title": (title_only, "marigold"), "description": (description_only, "periwinkle")}


def choose(values, total):
    """The most common value that still splits the issues, if any."""
    counts = {}
    for value in values:
        if value is not None:
            counts[value] = counts.get(value, 0) + 1
    if not counts:
        return None
    splitting = [value for value, count in counts.items() if count < total]
    pool = splitting or list(counts)
    return max(pool, key=lambda value: (counts[value], str(value)))


def check_filter(ctx, description, arguments, expected_ids, value):
    rec = ctx.rec
    if value is None:
        rec.skip(description, "no issue holds a value to filter on")
        return
    got = ids(ctx.list_all(limit=100, **arguments))
    if got == expected_ids:
        rec.check(description, True, f"{value!r}: {len(got)} issues")
    else:
        rec.check(description, False, f"{value!r}: missing {sorted(expected_ids - got)[:5]}, "
                                      f"extra {sorted(got - expected_ids)[:5]}")


def query_word(issues, code):
    words = {}
    for issue in issues:
        for word in issue["title"].lower().split():
            if word.isalpha() and len(word) >= 4 and word not in code.lower():
                words.setdefault(word, set()).add(issue["identifier"])
    splitting = [word for word, holders in words.items() if len(holders) < len(issues)]
    pool = splitting or list(words)
    return max(pool, key=lambda word: (len(words[word]), word)) if pool else None


def test_read(ctx):
    rec = ctx.rec
    if not ctx.read_only:
        seed_read_corpus(ctx)
    project = ctx.project_id
    issues, pages = ctx.list_pages(100, project_id=project)
    total = len(issues)
    rec.check("list_issues returns the team's issues", total > 0, f"{total} issues")
    if not issues:
        return
    rec.check("every listed issue belongs to the filtered team",
              all(issue["project_id"] == project for issue in issues), f"project {project}")
    times = [parse_timestamp(issue["updated_at"]) for issue in issues]
    rec.check("issues are ordered by updated_at, newest first",
              None not in times and times == sorted(times, reverse=True), f"{total} in order")

    size = 3 if total <= 60 else 25
    paged, paged_pages = ctx.list_pages(size, project_id=project)
    order = [issue["identifier"] for issue in issues]
    paged_order = [issue["identifier"] for issue in paged]
    rec.equal(f"cursor paging (limit {size}) visits every issue once, in order", paged_order, order)
    shapes = [len(page["issues"]) for page in paged_pages]
    rec.check("every page but the last is full and carries next_cursor",
              all(count == size and page.get("next_cursor") for count, page in
                  zip(shapes[:-1], paged_pages[:-1])) and not paged_pages[-1].get("next_cursor")
              and 0 < shapes[-1] <= size, f"pages {shapes}")
    if total > 1:
        one = ctx.list_page(project_id=project, limit=1)
        rec.check("a limit smaller than the matches returns next_cursor",
                  len(one["issues"]) == 1 and bool(one.get("next_cursor")),
                  f"{len(one['issues'])} issue, cursor {one.get('next_cursor')!r}")

    check_every(rec, "every issue record carries every field with its type", issues, record_problems)
    sample = issues[:400]
    mismatched = None
    for issue in sample:
        got = ctx.get(issue["identifier"])
        if got != issue:
            mismatched = f"{issue['identifier']}: {', '.join(changed_fields(issue, got))}"
            break
    rec.check("get_issue by key returns the same record list_issues does", mismatched is None,
              mismatched or f"{len(sample)} issues compared")
    check_every(rec, "state history is a contiguous audit ending in the current state",
                issues, history_problems)
    check_every(rec, "started/completed/canceled stamps match each state type",
                issues, lifecycle_problems)

    by_key = {issue["identifier"]: issue for issue in issues}

    def fetch(key):
        if key not in by_key:
            by_key[key] = ctx.get(key)
        return by_key[key]

    pairs = [(issue, relation) for issue in issues for relation in issue["relations"]]
    mirror = {"blocks": "blocked-by", "blocked-by": "blocks", "related": "related"}
    broken = None
    for issue, relation in pairs:
        other = fetch(relation["identifier"])
        wanted = (mirror.get(relation["kind"], "?"), issue["identifier"])
        if wanted not in relation_set(other):
            broken = f"{issue['identifier']} {relation['kind']} {relation['identifier']} is not mirrored"
            break
    if pairs:
        rec.check("every relation is seen from both ends", broken is None,
                  broken or f"{len(pairs)} relation ends")
    else:
        rec.skip("every relation is seen from both ends", "no relations in this team")

    types = ctx.state_types(project)
    parents = [issue for issue in issues if issue.get("children")]
    problem = None
    for parent in parents:
        children = parent["children"]
        sorts = [child["sort_order"] for child in children]
        done = sum(1 for child in children if types.get(child["state"]) in CLOSED_TYPES)
        if sorts != sorted(sorts):
            problem = f"{parent['identifier']} children are not in sort order"
        elif parent.get("sub_issues") != {"done": done, "total": len(children)}:
            problem = f"{parent['identifier']} progress {parent.get('sub_issues')} but {done} of {len(children)} closed"
        else:
            for child in children:
                record = fetch(child["identifier"])
                if record.get("parent") != parent["identifier"] or record["state"] != child["state"]:
                    problem = f"{child['identifier']} does not point back to {parent['identifier']}"
                    break
        if problem:
            break
    if not problem:
        for issue in issues:
            if issue.get("parent"):
                parent = fetch(issue["parent"])
                if issue["identifier"] not in [child["identifier"] for child in parent.get("children", [])]:
                    problem = f"{issue['parent']} does not list its child {issue['identifier']}"
                    break
    if parents or any(issue.get("parent") for issue in issues):
        rec.check("parents list children in order with progress, children link back",
                  problem is None, problem or f"{len(parents)} parents")
    else:
        rec.skip("parents list children in order with progress, children link back", "no sub-issues")

    duplicates = [issue for issue in issues if issue.get("duplicate_of")]
    problem = None
    for issue in duplicates:
        target = fetch(issue["duplicate_of"])
        if issue["state_type"] != "duplicate":
            problem = f"{issue['identifier']} is a duplicate in {issue['state']}"
        elif issue["identifier"] not in target.get("duplicates", []):
            problem = f"{target['identifier']} does not list duplicate {issue['identifier']}"
        if problem:
            break
    if duplicates:
        rec.check("duplicates sit in a duplicate state and their target lists them",
                  problem is None, problem or f"{len(duplicates)} duplicates")
    else:
        rec.skip("duplicates sit in a duplicate state and their target lists them", "no duplicates")

    everything = ids(ctx.list_all(limit=100))
    rec.check("a listing without a team spans every team", ids(issues) <= everything
              and (ctx.read_only or any(key.startswith(ctx.other_code + "-") for key in everything)),
              f"{len(everything)} issues across teams")
    check_filter(ctx, "team_id filters exactly like project_id", {"team_id": project}, ids(issues), project)

    state = choose([issue["state"] for issue in issues], total)
    expected = {issue["identifier"] for issue in issues if issue["state"] == state}
    check_filter(ctx, "state filter by name", {"project_id": project, "state": state}, expected, state)
    if state:
        check_filter(ctx, "state filter by name ignores case",
                     {"project_id": project, "state": state.lower()}, expected, state.lower())
    kind = choose([issue["state_type"] for issue in issues], total)
    check_filter(ctx, "state filter by state type", {"project_id": project, "state": kind},
                 {issue["identifier"] for issue in issues if issue["state_type"] == kind}, kind)
    label = choose([label for issue in issues for label in issue["labels"]], total)
    check_filter(ctx, "label filter", {"project_id": project, "label": label},
                 {issue["identifier"] for issue in issues
                  if label and label.casefold() in [held.casefold() for held in issue["labels"]]}, label)
    milestone = choose([issue.get("milestone_id") for issue in issues], total)
    check_filter(ctx, "milestone filter", {"project_id": project, "milestone_id": milestone},
                 {issue["identifier"] for issue in issues if issue.get("milestone_id") == milestone},
                 milestone)
    assignee = choose([issue.get("assignee") for issue in issues], total)
    check_filter(ctx, "assignee filter", {"project_id": project, "assignee": assignee},
                 {issue["identifier"] for issue in issues if issue.get("assignee") == assignee}, assignee)
    word = query_word(issues, ctx.project_code)
    check_filter(ctx, "text query matches title or description, ignoring case",
                 {"project_id": project, "query": word.upper() if word else None},
                 {issue["identifier"] for issue in issues if word and
                  (word in issue["title"].lower() or word in issue["description"].lower())}, word)
    examples = getattr(ctx, "query_examples", None)
    if examples:
        title_key, title_token = examples["title"]
        description_key, description_token = examples["description"]
        check_filter(ctx, "a title token matches only the issue whose title holds it",
                     {"project_id": project, "query": title_token.upper()}, {title_key}, title_token.upper())
        check_filter(ctx, "a description token matches only the issue whose description holds it",
                     {"project_id": project, "query": description_token.upper()},
                     {description_key}, description_token.upper())
    else:
        rec.skip("a title token matches only the issue whose title holds it", "no seeded query examples")
        rec.skip("a description token matches only the issue whose description holds it",
                 "no seeded query examples")
    stamps = sorted(issue["updated_at"] for issue in issues)
    since = stamps[len(stamps) // 2]
    boundary = parse_timestamp(since)
    check_filter(ctx, "updated_since keeps issues updated at or after the instant",
                 {"project_id": project, "updated_since": since},
                 {issue["identifier"] for issue in issues if parse_timestamp(issue["updated_at"]) >= boundary},
                 since)
    parent = max(parents, key=lambda record: len(record["children"]), default=None)
    if parent:
        children = {child["identifier"] for child in parent["children"]}
        check_filter(ctx, "parent_id filter by key lists the direct children",
                     {"parent_id": parent["identifier"]}, children, parent["identifier"])
        check_filter(ctx, "parent_id filter by numeric id lists the same children",
                     {"parent_id": str(parent["ticket_id"])}, children, parent["ticket_id"])
    else:
        rec.skip("parent_id filter lists the direct children", "no parent issues")
    if label and kind:
        check_filter(ctx, "filters combine with AND",
                     {"project_id": project, "state": kind, "label": label},
                     {issue["identifier"] for issue in issues if issue["state_type"] == kind
                      and label.casefold() in [held.casefold() for held in issue["labels"]]},
                     f"{kind} + {label}")


# Test 3: Update -------------------------------------------------------------

UPDATE_DESCRIPTION = (
    "Intro line.\n\nStatus: TODO\n\nDetails stay exactly as written, *including* this.\n\n"
    "Status: TODO\n"
)


def test_update(ctx):
    rec = ctx.rec
    key = ctx.create("T3 Original title", "LIN", description=UPDATE_DESCRIPTION, state="Draft",
                     priority=3, due_date="2030-04-01", add_labels=["frontend", "size-small"],
                     milestone_id=ctx.milestones["M1"])
    before = ctx.get(key)
    ctx.save_must(key, title="T3 New title")
    after = ctx.get(key)
    rec.equal("title change is stored", after["title"], "T3 New title")
    rec.check("an edit advances updated_at", parse_timestamp(after["updated_at"]) >
              parse_timestamp(before["updated_at"]), f"{before['updated_at']} -> {after['updated_at']}")
    rec.check("an edit advances the version", after["version"] > before["version"],
              f"{before['version']} -> {after['version']}")
    ctx.only_changed("a title change leaves every other field alone", before, after, ["title"])

    before = after
    ctx.save_must(key, state="Active")
    after = ctx.get(key)
    rec.equal("state change is stored with its type", (after["state"], after["state_type"]),
              ("Active", "started"))

    seen = {}
    for priority in (1, 4, 0, 2, 3):
        ctx.save_must(key, priority=priority)
        seen[priority] = ctx.get(key)["priority"]
    rec.equal("every priority 0-4 round-trips, including 0 (none)", seen, {p: p for p in seen})

    ctx.save_must(key, due_date="2030-05-02")
    rec.equal("due date change is stored", ctx.get(key).get("due_date"), "2030-05-02")
    ctx.save_must(key, title="T3 New title again")
    rec.equal("an update that omits due_date keeps it", ctx.get(key).get("due_date"), "2030-05-02")
    before = ctx.get(key)
    ctx.save_must(key, due_date=None)
    after = ctx.get(key)
    rec.check("due_date null clears the due date", "due_date" not in after, after.get("due_date", "cleared"))
    ctx.only_changed("clearing the due date leaves every other field alone", before, after, ["due_date"])

    before = after
    patch = '{"find":"Status: TODO","replace":"Status: DONE"}'
    ctx.save_must(key, description_patch=patch)
    after = ctx.get(key)
    rec.equal("description_patch replaces the first match and keeps the rest byte for byte",
              after["description"], UPDATE_DESCRIPTION.replace("Status: TODO", "Status: DONE", 1))
    ctx.only_changed("a description patch leaves every other field alone", before, after, ["description"])
    before = after
    result = ctx.save(key, description_patch='{"find":"no such text","replace":"x"}')
    ctx.refused("an unmatched description_patch is refused, naming the field", result,
                "invalid_request", "description_patch")
    ctx.unchanged("the refused patch changes nothing", key, before)

    ctx.save_must(key, add_labels=["storage"])
    after = ctx.get(key)
    rec.equal("add_labels adds without replacing the set", sorted(after["labels"]),
              ["frontend", "size-small", "storage"])
    rec.check("a label change advances updated_at",
              parse_timestamp(after["updated_at"]) > parse_timestamp(before["updated_at"]),
              f"{before['updated_at']} -> {after['updated_at']}")
    ctx.save_must(key, remove_labels=["frontend"])
    rec.equal("remove_labels removes only the named label", sorted(ctx.get(key)["labels"]),
              ["size-small", "storage"])
    ctx.save_must(key, add_labels=["size-large"])
    labels = ctx.get(key)["labels"]
    rec.equal("a single-select group holds only the newest of its labels",
              sorted(label for label in labels if label.startswith("size-")), ["size-large"])
    rec.check("replacing within a single-select group keeps other labels", "storage" in labels, str(labels))
    before = ctx.get(key)
    result = ctx.save(key, add_labels=["size-small", "size-medium"])
    ctx.refused("two labels from one single-select group in one write are refused",
                result, "invalid_request", "add_labels")
    ctx.unchanged("the refused label write changes nothing", key, before)
    ctx.save_must(key, add_labels=["docs", "testing"])
    labels = ctx.get(key)["labels"]
    rec.check("a multi-select group holds several of its labels",
              {"docs", "testing"} <= set(labels), str(sorted(labels)))

    ctx.save_must(key, milestone_id=ctx.milestones["M2"])
    rec.equal("milestone change is stored", ctx.get(key).get("milestone_id"), ctx.milestones["M2"])
    ctx.save_must(key, milestone_id=None)
    rec.check("milestone_id null clears the milestone", "milestone_id" not in ctx.get(key), "cleared")
    ctx.save_must(key, assignee="operator")
    rec.equal("assignee is stored", ctx.get(key).get("assignee"), "operator")
    ctx.save_must(key, assignee=None)
    rec.check("assignee null clears the assignee", "assignee" not in ctx.get(key), "cleared")
    before = ctx.get(key)
    ctx.save_must(key, title="T3 Combined", priority=1, add_labels=["platform"], due_date="2030-07-07")
    after = ctx.get(key)
    rec.check("one write can change several fields together",
              (after["title"], after["priority"], after.get("due_date")) == ("T3 Combined", 1, "2030-07-07")
              and "platform" in after["labels"], f"{after['title']}, {after['priority']}, {after.get('due_date')}")
    ctx.only_changed("the combined write moved only what it named", before, after,
                     ["title", "priority", "labels", "due_date"])

    # Label and state names match as Linear's do.
    scratch = ctx.create("T3 Case-insensitive label", "LIN")
    result = ctx.save(scratch, add_labels=["FRONTEND"])
    labels = ctx.get(scratch)["labels"]
    rec.check("add_labels matches a label name ignoring case (FRONTEND -> frontend)",
              result.ok and labels == ["frontend"], result.describe() if not result.ok else str(labels))
    scratch = ctx.create("T3 Lowercase state name", "LIN")
    result = ctx.save(scratch, state="done")
    state = ctx.get(scratch)["state"]
    rec.check("state matches a state name ignoring case (done -> Done)",
              result.ok and state == "Done", result.describe() if not result.ok else state)
    scratch = ctx.create("T3 State by type", "LIN")
    result = ctx.save(scratch, state="completed")
    record = ctx.get(scratch)
    rec.check("state accepts a state type (completed -> a completed state)",
              result.ok and record["state_type"] == "completed",
              result.describe() if not result.ok else record["state"])

    scratch = ctx.create("T3 No-op labels", "LIN", add_labels=["frontend"])
    before = ctx.get(scratch)
    ctx.save_must(scratch, add_labels=["frontend"])
    after = ctx.get(scratch)
    rec.check("adding a label the issue already holds moves nothing (updated_at, version)",
              (after["updated_at"], after["version"]) == (before["updated_at"], before["version"]),
              f"updated_at {before['updated_at']} -> {after['updated_at']}")
    before = after
    ctx.save_must(scratch, remove_labels=["storage"])
    after = ctx.get(scratch)
    rec.check("removing a label the issue does not hold moves nothing (updated_at, version)",
              (after["updated_at"], after["version"]) == (before["updated_at"], before["version"]),
              f"updated_at {before['updated_at']} -> {after['updated_at']}")

    # Legacy socket edits keep the issue model in step.
    legacy = ctx.legacy("ticket.create", {
        "mutation": ctx.mutation(0, "legacy-create"), "project_id": ctx.other_project_id,
        "kind": "task", "priority": "low", "title": "T3 Legacy edits", "subtype": "operational",
        "mode": "human", "completion": ["Done"],
    })
    legacy_key = f"{ctx.other_code}-{legacy['number']}"
    before = ctx.get(legacy_key)
    ctx.legacy("ticket.prioritise", {"mutation": ctx.mutation(before["version"], "legacy-priority"),
                                     "ticket_id": legacy["id"], "priority": "high"})
    after = ctx.get(legacy_key)
    rec.check("legacy ticket.prioritise sets the issue priority (high -> 2)",
              after["priority"] == 2, f"priority {after['priority']}")
    before = after
    ctx.legacy("ticket.edit", {"mutation": ctx.mutation(before["version"], "legacy-edit"),
                               "ticket_id": legacy["id"], "title": "T3 Legacy edits, renamed"})
    after = ctx.get(legacy_key)
    rec.equal("legacy ticket.edit changes the issue title", after["title"], "T3 Legacy edits, renamed")
    rec.check("legacy ticket.edit advances updated_at",
              parse_timestamp(after["updated_at"]) > parse_timestamp(before["updated_at"]),
              f"{before['updated_at']} -> {after['updated_at']}")


# Test 4: State moves ---------------------------------------------------------

def stamp_problems(record):
    """How the stamps on one record differ from Linear's rules after a move.

    Completed and canceled stamps mark the latest entry into that type.
    The started stamp must mark an entry into a started state; the model
    does not say whether a re-entry restamps it, so either is accepted.
    """
    kind = record["state_type"]
    rows = record["state_history"]
    entered = rows[-1]["started_at"]
    problems = lifecycle_problems(record)
    if kind == "completed" and record.get("completed_at") != entered:
        problems.append(f"completed_at {record.get('completed_at')} is not the entry {entered}")
    if kind == "canceled" and record.get("canceled_at") != entered:
        problems.append(f"canceled_at {record.get('canceled_at')} is not the entry {entered}")
    starts = {row["started_at"] for row in rows if row["state_type"] == "started"}
    if kind == "started" and record.get("started_at") not in starts:
        problems.append(f"started_at {record.get('started_at')} marks no entry into a started state")
    return problems


def test_state_moves(ctx):
    """Every workflow state can move to every other, with history and stamps."""
    rec = ctx.rec
    states = ctx.must("list_issue_statuses", {"project_id": ctx.project_id})["states"]
    for source in states:
        for destination in states:
            if source["name"] == destination["name"]:
                continue
            key = ctx.create(f"Move {source['name']} to {destination['name']}", "LIN",
                             state=source["name"])
            before = ctx.get(key)
            ctx.save_must(key, state=destination["name"])
            after = ctx.get(key)
            landed = (after["state"], after["state_type"]) == (destination["name"], destination["state_type"])
            rec.check(f"{source['name']} -> {destination['name']}: the move lands", landed,
                      f"{after['state']}/{after['state_type']}")
            problems = history_appended(before, after, destination["name"], destination["state_type"])
            if parse_timestamp(after["updated_at"]) <= parse_timestamp(before["updated_at"]):
                problems.append("updated_at did not advance")
            rec.check(f"{source['name']} -> {destination['name']}: one history row is appended",
                      not problems, "; ".join(problems) or f"{len(after['state_history'])} rows")
            problems = stamp_problems(after)
            rec.check(
                f"{source['name']} -> {destination['name']}: stamps follow the {destination['state_type']} type",
                not problems, "; ".join(problems) or stamp_summary(after),
            )

    stayed = ctx.create("Stay where it is", "LIN", state="Done")
    before = ctx.get(stayed)
    ctx.save_must(stayed, state="Done")
    after = ctx.get(stayed)
    rec.check("moving to the current state writes nothing",
              after["state_history"] == before["state_history"] and after["version"] == before["version"]
              and after["updated_at"] == before["updated_at"],
              f"{len(after['state_history'])} rows, version {after['version']}")


    for state, stamp in (("Active", "started_at"), ("Done", "completed_at"), ("Cancelled", "canceled_at")):
        created = ctx.get(ctx.create(f"T4 Born {state}", "LIN", state=state))
        rec.check(f"creating straight into {state} stamps {stamp} at creation",
                  created.get(stamp) == created["created_at"] and len(created["state_history"]) == 1
                  and not lifecycle_problems(created),
                  f"{stamp} {created.get(stamp)}, created {created['created_at']}")


def stamp_summary(record):
    parts = [f"{name.split('_')[0]} {record[name]}" for name in ("started_at", "completed_at", "canceled_at")
             if name in record]
    return ", ".join(parts) or "no stamps"


# Test 5: Relations -----------------------------------------------------------


def test_relations(ctx):
    rec = ctx.rec
    a = ctx.create("T5 Relation hub A", "LIN")
    b = ctx.create("T5 Blocked B", "LIN")
    c = ctx.create("T5 Blocked C", "LIN")
    d = ctx.create("T5 Related D", "LIN")
    x = ctx.create("T5 Other team X", "LTB")

    ctx.save_must(a, add_blocks=[b])
    rec.check("add_blocks: the blocker lists blocks", has_relation(ctx.get(a), "blocks", b), b)
    rec.check("add_blocks: the blocked issue lists blocked-by", has_relation(ctx.get(b), "blocked-by", a), a)
    ctx.save_must(c, add_blocked_by=[a])
    rec.check("add_blocked_by: the blocked issue lists blocked-by", has_relation(ctx.get(c), "blocked-by", a), a)
    rec.check("add_blocked_by: the blocker lists blocks", has_relation(ctx.get(a), "blocks", c), c)
    ctx.save_must(a, add_related=[d])
    rec.check("add_related: the issue lists related", has_relation(ctx.get(a), "related", d), d)
    rec.check("add_related: the other end lists related too", has_relation(ctx.get(d), "related", a), a)
    ctx.save_must(a, add_related=[x])
    rec.check("relations reach across teams, both ends",
              has_relation(ctx.get(a), "related", x) and has_relation(ctx.get(x), "related", a), x)
    ctx.save_must(a, add_blocks=[b])
    count = sum(1 for relation in ctx.get(a)["relations"] if relation == {"kind": "blocks", "identifier": b})
    rec.equal("adding an existing relation again does not duplicate it", count, 1)

    ctx.save_must(a, remove_blocks=[b])
    rec.check("remove_blocks: gone from the blocker", not has_relation(ctx.get(a), "blocks", b), b)
    rec.check("remove_blocks: gone from the blocked issue", not has_relation(ctx.get(b), "blocked-by", a), a)
    ctx.save_must(c, remove_blocked_by=[a])
    rec.check("remove_blocked_by: gone from both ends",
              not has_relation(ctx.get(c), "blocked-by", a) and not has_relation(ctx.get(a), "blocks", c), c)
    rec.check("removing one relation leaves the others", has_relation(ctx.get(a), "related", d)
              and has_relation(ctx.get(a), "related", x), str(sorted(relation_set(ctx.get(a)))))
    ctx.save_must(a, remove_related=[x])
    rec.check("remove_related from the adding end: gone from both ends",
              not has_relation(ctx.get(a), "related", x) and not has_relation(ctx.get(x), "related", a), x)
    result = ctx.save(d, remove_related=[a])
    after_a, after_d = ctx.get(a), ctx.get(d)
    rec.check("remove_related from the other end: gone from both ends (related is undirected)",
              result.ok and not has_relation(after_a, "related", d) and not has_relation(after_d, "related", a),
              result.describe() if not result.ok else
              f"{a} still lists {sorted(relation_set(after_a, 'related'))}, "
              f"{d} still lists {sorted(relation_set(after_d, 'related'))}")

    e = ctx.create("T5 Mentioner E", "LIN", description=f"Follow-up of {a} (see notes), and {ctx.project_code}-999999.")
    rec.check("mentioning a key in a description adds related on the mentioner",
              has_relation(ctx.get(e), "related", a), a)
    rec.check("...and on the mentioned issue", has_relation(ctx.get(a), "related", e), e)
    rec.check("mentioning an unknown key adds nothing",
              not any(relation["identifier"].endswith("-999999") for relation in ctx.get(e)["relations"]),
              str(sorted(relation_set(ctx.get(e)))))
    ctx.save_must(e, description_patch='{"find":"see notes","replace":"see ' + b + '"}')
    rec.check("a description patch that mentions a key adds related too",
              has_relation(ctx.get(e), "related", b), b)
    ctx.save_must(e, description=f"This is {e} itself.")
    rec.check("mentioning the issue's own key adds no self-relation",
              not has_relation(ctx.get(e), "related", e), str(sorted(relation_set(ctx.get(e)))))

    before = ctx.get(a)
    for field in ("add_blocks", "add_blocked_by", "add_related"):
        result = ctx.save(a, **{field: [a]})
        ctx.refused(f"{field} naming the issue itself is refused", result, "invalid_request", field)
    ctx.unchanged("the refused self-relations change nothing", a, before)


# Test 6: Duplicate merge -----------------------------------------------------


def test_duplicate_merge(ctx):
    rec = ctx.rec
    target = ctx.create("T6 Canonical issue", "LIN", state="Active")
    dup = ctx.create("T6 Duplicate issue", "LIN", state="Ready")
    p = ctx.create("T6 Blocked by the duplicate", "LIN")
    q = ctx.create("T6 Blocks the duplicate", "LIN")
    r = ctx.create("T6 Related to the duplicate", "LIN")
    ctx.save_must(dup, add_blocks=[p], add_blocked_by=[q], add_related=[r, target])
    before = ctx.get(dup)
    ctx.save_must(dup, duplicate_of=target)
    after = ctx.get(dup)
    rec.equal("duplicate_of moves the issue to the Duplicate state",
              (after["state"], after["state_type"]), ("Duplicate", "duplicate"))
    problems = history_appended(before, after, "Duplicate", "duplicate")
    rec.check("the move to Duplicate is appended to the history", not problems, "; ".join(problems) or "appended")
    rec.check("entering Duplicate stamps canceled_at", after.get("canceled_at") ==
              after["state_history"][-1]["started_at"] and "completed_at" not in after, stamp_summary(after))
    rec.equal("the duplicate names its target", after.get("duplicate_of"), target)
    canonical = ctx.get(target)
    rec.check("the target lists the duplicate", dup in canonical.get("duplicates", []),
              str(canonical.get("duplicates")))
    rec.check("blocks moves onto the target",
              sum(1 for rel in canonical["relations"] if rel == {"kind": "blocks", "identifier": p}) == 1, p)
    blocked = ctx.get(p)
    rec.check("the blocked issue now lists the target, not the duplicate",
              has_relation(blocked, "blocked-by", target) and not has_relation(blocked, "blocked-by", dup),
              str(sorted(relation_set(blocked))))
    rec.check("blocked-by moves onto the target, both ends",
              has_relation(canonical, "blocked-by", q) and has_relation(ctx.get(q), "blocks", target)
              and not has_relation(ctx.get(q), "blocks", dup), q)
    rec.check("related moves onto the target, both ends",
              has_relation(canonical, "related", r) and has_relation(ctx.get(r), "related", target)
              and not has_relation(ctx.get(r), "related", dup), r)
    rec.check("the target gains no relation to itself",
              not any(rel["identifier"] == target for rel in canonical["relations"]),
              str(sorted(relation_set(canonical))))
    rec.equal("the duplicate keeps no relations of its own", after["relations"], [])

    before = ctx.get(dup)
    result = ctx.save(dup, duplicate_of=dup)
    ctx.refused("an issue cannot duplicate itself", result, "invalid_request", "duplicate_of")
    result = ctx.save(dup, duplicate_of=f"{ctx.project_code}-999999")
    ctx.refused("duplicate_of an unknown key is not found", result, "not_found")
    ctx.unchanged("the refused duplicate writes change nothing", dup, before)
    ctx.save_must(dup, duplicate_of=None)
    rec.check("duplicate_of null clears the link on both ends",
              "duplicate_of" not in ctx.get(dup) and dup not in ctx.get(target).get("duplicates", []),
              "cleared")
    ctx.save_must(dup, state="Active")
    ctx.save_must(dup, duplicate_of=target)
    rec.equal("marking it duplicate again re-enters Duplicate", ctx.get(dup)["state"], "Duplicate")

    # The duplicate and its target both block the same issue: the merge
    # must fold the two edges into one rather than fail.
    shared_target = ctx.create("T6 Canonical with a shared blocker edge", "LIN")
    shared_dup = ctx.create("T6 Duplicate with a shared blocker edge", "LIN")
    shared = ctx.create("T6 Blocked by both", "LIN")
    ctx.save_must(shared_dup, add_blocks=[shared])
    ctx.save_must(shared_target, add_blocks=[shared])
    result = ctx.save(shared_dup, duplicate_of=shared_target)
    merged = ctx.get(shared_target)
    edges = sum(1 for rel in merged["relations"] if rel == {"kind": "blocks", "identifier": shared})
    rec.check("a merge where both issues block the same issue keeps one edge",
              result.ok and ctx.get(shared_dup)["state"] == "Duplicate" and edges == 1
              and not has_relation(ctx.get(shared), "blocked-by", shared_dup),
              result.describe() if not result.ok else f"{edges} blocks edge to {shared}")


# Test 7: Split ---------------------------------------------------------------


def milestone_counts(ctx):
    milestones = ctx.must("list_milestones", {"project_id": ctx.project_id})["milestones"]
    return {milestone["id"]: milestone["issues_total"] for milestone in milestones}


def test_split(ctx):
    rec = ctx.rec
    pl1, pl2 = ctx.milestones["M1"], ctx.milestones["M2"]
    original = ctx.create("T7 Big piece of work", "LIN", description="Part one.\n\nPart two.\n",
                          milestone_id=pl1, add_labels=["frontend"], state="Ready")
    counts_before = milestone_counts(ctx)
    before = ctx.get(original)
    first = ctx.create("T7 Part one", "LIN", description="Part one.\n", milestone_id=pl1,
                       add_related=[original])
    second = ctx.create("T7 Part two", "LIN", description="Part two.\n", milestone_id=pl1,
                        add_related=[original], add_blocked_by=[first])
    rec.equal("the two new issues take the next keys", key_number(second), key_number(first) + 1)
    linked = ctx.get(original)
    rec.check("both halves are linked to the original from both ends",
              all(has_relation(linked, "related", key) and has_relation(ctx.get(key), "related", original)
                  for key in (first, second)), str(sorted(relation_set(linked))))
    rec.check("relations sent on create land on both ends",
              has_relation(ctx.get(first), "blocks", second) and has_relation(ctx.get(second), "blocked-by", first),
              f"{first} blocks {second}")
    ctx.save_must(original, title="T7 Big piece of work (split)", milestone_id=pl2)
    after = ctx.get(original)
    rec.equal("the original is retitled", after["title"], "T7 Big piece of work (split)")
    rec.equal("the original is re-homed to the other milestone", after.get("milestone_id"), pl2)
    ctx.only_changed("the split leaves the original's other fields alone", before, after,
                     ["title", "milestone_id", "relations"])
    rec.equal("retitling and re-homing add no state history", after["state_history"], before["state_history"])
    in_pl2 = ids(ctx.list_all(project_id=ctx.project_id, milestone_id=pl2))
    in_pl1 = ids(ctx.list_all(project_id=ctx.project_id, milestone_id=pl1))
    rec.check("milestone filters see the move", original in in_pl2 and original not in in_pl1
              and {first, second} <= in_pl1, f"{original} in M2")
    counts_after = milestone_counts(ctx)
    rec.equal("milestone progress counts follow the issues",
              (counts_after[pl1] - counts_before[pl1], counts_after[pl2] - counts_before[pl2]), (1, 1))


# Test 8: Comments ------------------------------------------------------------


def comment_target(ctx, key):
    return {"kind": "ticket", "id": str(ctx.get(key)["ticket_id"])}


def list_comments(ctx, target, limit=0):
    comments, cursor, pages = [], None, 0
    while True:
        arguments = {"target": target}
        if limit:
            arguments["limit"] = limit
        if cursor:
            arguments["cursor"] = cursor
        page = ctx.must("list_comments", arguments)
        comments.extend(page["comments"])
        cursor, pages = page.get("next_cursor"), pages + 1
        if not cursor or pages > 1000:
            return comments


def test_comments(ctx):
    rec = ctx.rec
    report = (FIXTURES / "long-report.md").read_text(encoding="utf-8")
    key = ctx.create("T8 Comment host", "LIN")
    other = ctx.create("T8 Quiet neighbour", "LIN")
    target = comment_target(ctx, key)
    created = ctx.must("comment_create", {"mutation": ctx.mutation(0, "comment"), "project_id": ctx.project_id,
                                          "author": "Reporter", "target": target, "text": report})
    comment_id = created["id"]
    rec.check("comment_create returns the new comment at version 1",
              created.get("version") == 1 and created.get("text") == report, f"id {comment_id}")
    listed = {comment["id"]: comment for comment in list_comments(ctx, target)}
    landed = listed.get(comment_id, {})
    rec.equal(f"a {len(report)}-character Markdown report lands word for word", landed.get("body"), report)
    rec.check("the comment carries its author and creation time, unedited and unresolved",
              landed.get("author") == "Reporter" and parse_timestamp(landed.get("created_at")) is not None
              and not {"edited_at", "resolved_at", "parent_comment_id"} & set(landed),
              f"author {landed.get('author')!r}")
    spaced = "  Leading spaces stay.\n\nTrailing blank lines stay too.\n\n\n"
    spaced_id = ctx.must("comment_create", {"mutation": ctx.mutation(0, "comment"), "project_id": ctx.project_id,
                                            "author": "Reporter", "target": target, "text": spaced})["id"]
    bodies = {comment["id"]: comment["body"] for comment in list_comments(ctx, target)}
    rec.equal("leading spaces and trailing newlines are not trimmed", bodies.get(spaced_id), spaced)

    edited = report.replace("**mostly ready**", "**ready**")
    ctx.tick()
    ctx.must("comment_edit", {"mutation": ctx.mutation(1, "comment-edit"), "comment_id": comment_id,
                              "text": edited})
    landed = {comment["id"]: comment for comment in list_comments(ctx, target)}.get(comment_id, {})
    rec.equal("an edit replaces the body", landed.get("body"), edited)
    rec.check("an edited comment shows when it was edited",
              parse_timestamp(landed.get("edited_at")) is not None and
              parse_timestamp(landed.get("edited_at")) >= parse_timestamp(landed.get("created_at")),
              str(landed.get("edited_at")))
    revisions = ctx.must("comment_revisions", {"comment_id": comment_id})["revisions"]
    rec.equal("revisions keep the original and the edit, in order",
              [revision["text"] == text for revision, text in zip(revisions, (report, edited))] +
              [len(revisions)], [True, True, 2])
    result = ctx.call("comment_edit", {"mutation": ctx.mutation(1, "comment-edit"), "comment_id": comment_id,
                                       "text": "stale"})
    ctx.refused("an edit at a stale version is refused", result, "stale_version")
    body = {comment["id"]: comment["body"] for comment in list_comments(ctx, target)}.get(comment_id)
    rec.equal("the stale edit changes nothing", body, edited)

    reply = ctx.must("comment_create", {"mutation": ctx.mutation(0, "reply"), "project_id": ctx.project_id,
                                        "author": "operator", "target": target, "text": "Thanks, noted.",
                                        "parent_comment_id": comment_id})["id"]
    landed = {comment["id"]: comment for comment in list_comments(ctx, target)}
    rec.equal("a reply is threaded under its parent", landed.get(reply, {}).get("parent_comment_id"), comment_id)
    version = landed[comment_id]["version"]
    resolved = ctx.must("resolve_comment", {"mutation": ctx.mutation(version, "resolve"),
                                            "comment_id": comment_id, "resolved": True})
    landed = {comment["id"]: comment for comment in list_comments(ctx, target)}.get(comment_id, {})
    rec.check("resolving a thread records when it was resolved",
              resolved.get("resolved") is True and parse_timestamp(landed.get("resolved_at")) is not None,
              str(landed.get("resolved_at")))
    ctx.must("resolve_comment", {"mutation": ctx.mutation(resolved["version"], "unresolve"),
                                 "comment_id": comment_id, "resolved": False})
    landed = {comment["id"]: comment for comment in list_comments(ctx, target)}.get(comment_id, {})
    rec.check("unresolving clears the resolution", "resolved_at" not in landed,
              landed.get("resolved_at", "cleared"))

    for index in range(3):
        ctx.must("comment_create", {"mutation": ctx.mutation(0, "comment"), "project_id": ctx.project_id,
                                    "author": "Reporter", "target": target, "text": f"Progress note {index + 1}."})
    everything = list_comments(ctx, target)
    order = [comment["id"] for comment in everything]
    times = [parse_timestamp(comment["created_at"]) for comment in everything]
    rec.check("comments list oldest first", order == sorted(order) and times == sorted(times),
              f"{len(order)} comments")
    paged = [comment["id"] for comment in list_comments(ctx, target, limit=2)]
    rec.equal("cursor paging (limit 2) visits every comment once, in order", paged, order)
    rec.equal("another issue's comments stay separate", list_comments(ctx, comment_target(ctx, other)), [])

    result = ctx.call("comment_create", {"mutation": ctx.mutation(0, "comment"), "project_id": ctx.project_id,
                                         "author": "Reporter", "target": target, "text": " \n\t "})
    ctx.refused("blank comment text is refused, naming the field", result, "invalid_request", "text")
    result = ctx.call("comment_create", {"mutation": ctx.mutation(0, "comment"), "project_id": ctx.project_id,
                                         "author": "Reporter", "target": {"kind": "ticket", "id": "999999"},
                                         "text": "Lost"})
    ctx.refused("a comment on an unknown issue is not found", result, "not_found")
    result = ctx.call("comment_create", {"mutation": ctx.mutation(0, "comment"),
                                         "project_id": ctx.other_project_id, "author": "Reporter",
                                         "target": target, "text": "Wrong team"})
    ctx.refused("a comment filed under the wrong team is refused, naming the field", result,
                "invalid_request", "project_id")
    rec.equal("the refused comments add nothing", [comment["id"] for comment in list_comments(ctx, target)], order)


# Test 9: Lookups -------------------------------------------------------------


def test_lookups(ctx):
    rec = ctx.rec
    project = ctx.project_id
    states = ctx.must("list_issue_statuses", {"project_id": project})["states"]
    names = [state.get("name") for state in states]
    kinds = {state.get("state_type") for state in states}
    rec.check("list_issue_statuses gives each state a name and a Linear type",
              bool(states) and all(isinstance(name, str) and name for name in names) and kinds <= STATE_TYPES,
              ", ".join(f"{state.get('name')}/{state.get('state_type')}" for state in states))
    rec.check("the workflow covers backlog, unstarted, started, completed and canceled",
              {"backlog", "unstarted", "started", "completed", "canceled"} <= kinds,
              "missing " + ", ".join(sorted({"backlog", "unstarted", "started", "completed", "canceled"} - kinds))
              if not {"backlog", "unstarted", "started", "completed", "canceled"} <= kinds else "all present")
    rec.check("the workflow has a duplicate-type state", "duplicate" in kinds, str(sorted(kinds)))
    rec.equal("state names are unique", len(set(names)), len(names))
    issues = ctx.list_all(project_id=project)
    mapping = {state["name"]: state["state_type"] for state in states}
    stray = [issue["identifier"] for issue in issues if mapping.get(issue["state"]) != issue["state_type"]]
    rec.check("every issue sits in a listed state of the matching type", not stray,
              f"{stray[:5]}" if stray else f"{len(issues)} issues")

    labels = ctx.must("list_issue_labels", {"project_id": project})["labels"]
    shape = [label for label in labels if not (isinstance(label.get("name"), str) and
             COLOUR_PATTERN.match(label.get("color", "")) and isinstance(label.get("group_single_select"), bool)
             and isinstance(label.get("retired"), bool))]
    rec.check("labels carry a name, a #RRGGBB colour and their flags", bool(labels) and not shape,
              f"{len(labels)} labels" if not shape else str(shape[:2]))
    groups = {}
    for label in labels:
        if label.get("group"):
            groups.setdefault(label["group"], set()).add(label["group_single_select"])
    rec.check("every label in a group agrees on single-select",
              all(len(flags) == 1 for flags in groups.values()),
              ", ".join(f"{group}: {'single' if flags == {True} else 'multi' if flags == {False} else 'mixed'}"
                        for group, flags in sorted(groups.items())) or "no groups")
    retired = sorted(label["name"] for label in labels if label["retired"])
    if retired or not ctx.read_only:
        rec.check("retired labels are still listed, flagged retired", bool(retired), str(retired))
    else:
        rec.skip("retired labels are still listed, flagged retired", "no retired labels in this team")
    known = {label["name"] for label in labels}
    held = {label for issue in issues for label in issue["labels"]}
    rec.check("every label an issue holds is listed for its team", held <= known,
              "unlisted: " + ", ".join(sorted(held - known)) if held - known else f"{len(held)} labels held")

    projects = ctx.must("list_projects", {})["projects"]
    codes = [entry.get("code") for entry in projects]
    rec.check("list_projects lists the team with its id, code and name",
              any(entry.get("id") == project and entry.get("code") == ctx.project_code and entry.get("name")
                  for entry in projects), ", ".join(str(code) for code in codes))
    rec.check("project ids and codes are unique",
              len({entry.get("id") for entry in projects}) == len(projects) == len(set(codes)),
              f"{len(projects)} projects")

    milestones = ctx.must("list_milestones", {"project_id": project})["milestones"]
    if milestones:
        bad = [m for m in milestones if not (isinstance(m.get("id"), int) and m.get("name")
               and 0 <= m.get("issues_done", -1) <= m.get("issues_total", -1)
               and ("target_date" not in m or DATE_PATTERN.match(m["target_date"])))]
        rec.check("milestones carry an id, a name, a target date and progress counts", not bad,
                  str(bad[:2]) if bad else f"{len(milestones)} milestones")
        wrong = []
        for milestone in milestones:
            members = [issue for issue in issues if issue.get("milestone_id") == milestone["id"]]
            done = sum(1 for issue in members if issue["state_type"] in CLOSED_TYPES)
            if (milestone["issues_total"], milestone["issues_done"]) != (len(members), done):
                wrong.append(f"{milestone['name']}: says {milestone['issues_done']}/{milestone['issues_total']}, "
                             f"issues give {done}/{len(members)}")
        rec.check("milestone progress counts the issues in it and the closed ones", not wrong,
                  "; ".join(wrong[:3]) or ", ".join(f"{m['name']} {m['issues_done']}/{m['issues_total']}"
                                                    for m in milestones))
    else:
        rec.skip("milestones carry an id, a name, a target date and progress counts", "no milestones")

    if ctx.read_only:
        return
    by_name = {label["name"]: label for label in labels}
    expected = {
        "size-small": ("Size", True), "size-medium": ("Size", True),
        "size-large": ("Size", True), "docs": ("Topic", False), "testing": ("Topic", False),
        "frontend": (None, False),
    }
    rec.equal("created labels keep their group and single-select flag",
              {name: (by_name.get(name, {}).get("group"), by_name.get(name, {}).get("group_single_select"))
               for name in expected}, expected)
    rec.equal("the retired label is the one retired in setup", retired, ["legacy-tag"])
    rec.check("an issue keeps a label retired after it was added",
              "legacy-tag" in ctx.get(ctx.retired_holder)["labels"], ctx.retired_holder)
    listed = [(m["name"], m.get("target_date")) for m in milestones]
    rec.equal("milestones list in creation order with their target dates", listed,
              [("M1 Staging", "2030-03-31"), ("M2 Launch", "2030-06-30")])
    other_labels = {label["name"] for label in ctx.must("list_issue_labels",
                                                         {"project_id": ctx.other_project_id})["labels"]}
    other_milestones = [m["name"] for m in ctx.must("list_milestones",
                                                    {"project_id": ctx.other_project_id})["milestones"]]
    rec.check("each team keeps its own labels and milestones",
              "size-small" not in other_labels and other_milestones == ["B1 Other team milestone"],
              f"{ctx.other_code}: {sorted(other_labels)}, {other_milestones}")
    rec.check("list_projects lists both teams", {ctx.project_code, ctx.other_code} <= set(codes), str(codes))


# Test 10: Errors -------------------------------------------------------------


def issue_count(ctx):
    return len(ctx.list_all(limit=100, project_id=ctx.project_id))


def test_errors(ctx):
    rec = ctx.rec
    code = ctx.project_code
    rec.equal("initialize without a credential is refused with HTTP 401",
              ctx.mcp.unauthorised_initialize_status(), 401)
    result = ctx.call("get_issue", {"identifier": f"{code}-999999"})
    ctx.refused("an unknown key is not found", result, "not_found")
    rec.check("the not-found message names the key", f"{code}-999999" in str((result.error or {}).get("message")),
              str((result.error or {}).get("message")))
    ctx.refused("an unknown team code is not found", ctx.call("get_issue", {"identifier": "ZZZ-1"}), "not_found")
    result = ctx.call("get_issue", {"identifier": "not a key"})
    rec.check("a malformed key gives a clear error", not result.ok and result.code in ("not_found", "invalid_request"),
              result.describe())
    ctx.refused("a malformed updated_since is refused, naming the field",
                ctx.call("list_issues", {"project_id": ctx.project_id, "updated_since": "yesterday"}),
                "invalid_request", "updated_since")
    ctx.refused("a malformed cursor is refused, naming the field",
                ctx.call("list_issues", {"project_id": ctx.project_id, "cursor": "not-a-cursor"}),
                "invalid_request", "cursor")
    for tool in ("list_issue_statuses", "list_issue_labels", "list_milestones"):
        ctx.refused(f"{tool} for an unknown team is not found",
                    ctx.call(tool, {"project_id": 999999}), "not_found")
    ctx.refused("list_issues for an unknown team is not found",
                ctx.call("list_issues", {"project_id": 999999}), "not_found")
    if ctx.read_only:
        rec.skip("refused writes leave the issue unchanged", "read-only mode sends no writes")
        return

    key = ctx.create("T10 Error target", "LIN", milestone_id=ctx.milestones["M1"], add_labels=["frontend"],
                     due_date="2030-08-08")
    count = issue_count(ctx)
    before = ctx.get(key)

    result = ctx.save(f"{code}-999999", version=0, title="Nobody")
    ctx.refused("updating an unknown key is not found", result, "not_found")
    rec.equal("the refused update creates nothing", issue_count(ctx), count)

    result = ctx.save(key, milestone_id=ctx.milestones["B1"])
    ctx.refused("a milestone from another team is refused, naming the field", result,
                "invalid_request", "milestone_id")
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, title="T10 Should not land", milestone_id=ctx.milestones["B1"])
    ctx.refused("a write mixing a good title and a bad milestone is refused", result,
                "invalid_request", "milestone_id")
    ctx.unchanged("...and the title does not land either (all or nothing)", key, before)
    result = ctx.save(key, add_labels=["legacy-tag"])
    ctx.refused("adding a retired label is refused, naming the field", result, "invalid_request", "add_labels")
    rec.check("the retired-label message says it is retired", "retired" in str((result.error or {}).get("message")),
              str((result.error or {}).get("message")))
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, add_labels=["no-such-label"])
    ctx.refused("adding an unknown label is refused, naming the field", result, "invalid_request", "add_labels")
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, add_related=[f"{code}-999999"])
    ctx.refused("a relation to an unknown key is not found", result, "not_found")
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, priority=5)
    ctx.refused("priority 5 is refused, naming the field", result, "invalid_request", "priority")
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, due_date="2030-02-30")
    ctx.refused("an impossible due date is refused, naming the field", result, "invalid_request", "due_date")
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, state="No Such State")
    ctx.refused("an unknown state is refused, naming the field", result, "invalid_request", "state")
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, version=before["version"] + 7, title="T10 Stale")
    rec.check("a stale version is refused with the current version",
              not result.ok and result.code == "stale_version"
              and (result.error or {}).get("current_version") == before["version"], result.describe())
    ctx.unchanged("...and changes nothing", key, before)
    result = ctx.save(key, title="   ")
    ctx.refused("a blank title is refused, naming the field", result, "invalid_request", "title")
    ctx.unchanged("...and changes nothing", key, before)

    for description, fields, field in (
        ("creating with a retired label is refused", {"project_id": ctx.project_id, "add_labels": ["legacy-tag"]},
         "add_labels"),
        ("creating with another team's milestone is refused",
         {"project_id": ctx.project_id, "milestone_id": ctx.milestones["B1"]}, "milestone_id"),
        ("creating without a team or parent is refused", {}, "project_id"),
    ):
        arguments = {"mutation": ctx.mutation(0, "create"), "title": "T10 Must not exist"}
        arguments.update(fields)
        ctx.refused(description + ", naming the field", ctx.call("save_issue", arguments),
                    "invalid_request", field)
    rec.equal("the refused creates add no issue", issue_count(ctx), count)
    ctx.refused("creating a milestone in an unknown team is not found, naming the field",
                ctx.call("create_milestone", {"mutation": ctx.mutation(0, "milestone"), "project_id": 999999,
                                              "name": "Nowhere"}), "not_found", "project_id")
    ctx.refused("retiring an already retired label is refused, naming the field",
                ctx.call("retire_label", {"mutation": ctx.mutation(0, "retire"), "project_id": ctx.project_id,
                                          "name": "legacy-tag"}), "invalid_request", "name")

    before = ctx.get(key)
    result = ctx.save(key, remove_labels=["no-such-label"])
    rec.check("removing an unknown label is not found, naming remove_labels",
              not result.ok and result.code == "not_found" and result.field == "remove_labels",
              result.describe())
    after = ctx.get(key)
    rec.check("...and changes nothing", not changed_fields(before, after),
              "changed: " + ", ".join(changed_fields(before, after)) if changed_fields(before, after) else "unchanged")


# Test 11: Sub-issues ---------------------------------------------------------


def child_keys(record):
    return [child["identifier"] for child in record.get("children", [])]


def test_sub_issues(ctx):
    rec = ctx.rec
    pl1, pl2 = ctx.milestones["M1"], ctx.milestones["M2"]
    parent = ctx.create("T11 Parent", "LIN", milestone_id=pl1, state="Active", priority=2)
    one = ctx.create("T11 Child one", parent_id=parent)
    two = ctx.create("T11 Child two", parent_id=parent, milestone_id=pl2)
    three = ctx.create("T11 Child three", "LIN", parent_id=parent, priority=4, add_labels=["storage"])
    first = ctx.get(one)
    rec.equal("a sub-issue created without a team inherits the parent's", first["project_id"], ctx.project_id)
    rec.equal("a sub-issue inherits the parent's milestone", first.get("milestone_id"), pl1)
    rec.equal("a sub-issue can override the milestone", ctx.get(two).get("milestone_id"), pl2)
    rec.check("a sub-issue is a full issue with its own key and state",
              KEY_PATTERN.match(one) and one != parent and first["state"] == "Draft"
              and first.get("parent") == parent, f"{one} in {first['state']}, parent {first.get('parent')}")
    third = ctx.get(three)
    rec.check("a sub-issue keeps its own priority and labels",
              third["priority"] == 4 and third["labels"] == ["storage"], f"priority {third['priority']}, {third['labels']}")
    record = ctx.get(parent)
    rec.equal("the parent lists its sub-issues in creation order", child_keys(record), [one, two, three])
    rec.check("each listed sub-issue carries its key, title, state and sort order",
              all({"identifier", "title", "state", "sort_order"} <= set(child) for child in record["children"]),
              str(record["children"][0]) if record.get("children") else "none")
    rec.equal("the parent shows progress (0 of 3 done)", record.get("sub_issues"), {"done": 0, "total": 3})

    ctx.save_must(three, sort_order=-1.0)
    rec.equal("sort_order moves a sub-issue to the front", child_keys(ctx.get(parent)), [three, one, two])
    ctx.save_must(two, sort_order=-0.5)
    rec.equal("a fractional sort_order slots between siblings", child_keys(ctx.get(parent)), [three, two, one])
    listed = ids(ctx.list_all(parent_id=parent))
    rec.equal("list_issues parent_id returns exactly the sub-issues", listed, {one, two, three})

    grandchild = ctx.create("T11 Grandchild", parent_id=one)
    rec.check("sub-issues nest several levels deep",
              ctx.get(grandchild).get("parent") == one and child_keys(ctx.get(one)) == [grandchild]
              and grandchild not in child_keys(ctx.get(parent)), f"{parent} > {one} > {grandchild}")
    before = ctx.get(parent)
    result = ctx.save(parent, parent_id=grandchild)
    ctx.refused("making an issue its own ancestor is refused, naming the field", result,
                "invalid_request", "parent_id")
    result = ctx.save(parent, parent_id=parent)
    ctx.refused("making an issue its own parent is refused, naming the field", result,
                "invalid_request", "parent_id")
    ctx.unchanged("the refused re-parents change nothing", parent, before)

    ctx.save_must(one, state="Done")
    rec.equal("progress counts a completed sub-issue (1 of 3)", ctx.get(parent).get("sub_issues"),
              {"done": 1, "total": 3})
    ctx.save_must(two, state="Cancelled")
    rec.equal("progress counts a canceled sub-issue as closed (2 of 3)", ctx.get(parent).get("sub_issues"),
              {"done": 2, "total": 3})
    settings = ctx.must("update_project_settings", {"mutation": ctx.mutation(0, "settings"),
                                                    "project_id": ctx.project_id})
    rec.equal("auto-close is off by default", settings.get("auto_close_sub_issues"), False)
    ctx.save_must(three, state="Done")
    record = ctx.get(parent)
    rec.check("with auto-close off, closing the last sub-issue leaves the parent open",
              record["state"] == "Active" and record.get("sub_issues") == {"done": 3, "total": 3},
              f"{record['state']}, {record.get('sub_issues')}")

    other = ctx.create("T11 Second parent", "LIN")
    ctx.save_must(two, parent_id=other)
    rec.check("re-parenting moves the sub-issue under the new parent",
              ctx.get(two).get("parent") == other and two in child_keys(ctx.get(other))
              and two not in child_keys(ctx.get(parent)), f"{two} under {other}")
    ctx.save_must(two, parent_id=None)
    record = ctx.get(two)
    rec.check("parent_id null makes it a top-level issue again",
              "parent" not in record and "children" not in ctx.get(other), record.get("parent", "top-level"))

    try:
        settings = ctx.must("update_project_settings", {"mutation": ctx.mutation(0, "settings"),
                                                        "project_id": ctx.project_id,
                                                        "auto_close_sub_issues": True})
        rec.equal("the auto-close setting can be turned on", settings.get("auto_close_sub_issues"), True)
        closing = ctx.create("T11 Auto-close parent", "LIN", state="Active")
        first_child = ctx.create("T11 Auto-close child one", parent_id=closing, state="Active")
        second_child = ctx.create("T11 Auto-close child two", parent_id=closing, state="Active")
        ctx.save_must(first_child, state="Done")
        rec.equal("auto-close waits while a sub-issue is open", ctx.get(closing)["state"], "Active")
        before = ctx.get(closing)
        ctx.save_must(second_child, state="Done")
        after = ctx.get(closing)
        rec.equal("closing the last sub-issue closes the parent", (after["state"], after["state_type"]),
                  ("Done", "completed"))
        problems = history_appended(before, after, "Done", "completed") + lifecycle_problems(after)
        rec.check("the automatic close is recorded like any state move", not problems,
                  "; ".join(problems) or stamp_summary(after))
        ctx.save_must(second_child, state="Active")
        ctx.save_must(second_child, state="Done")
        again = ctx.get(closing)
        rec.equal("an already closed parent is not closed again", len(again["state_history"]),
                  len(after["state_history"]))
    finally:
        ctx.must("update_project_settings", {"mutation": ctx.mutation(0, "settings"),
                                             "project_id": ctx.project_id, "auto_close_sub_issues": False})


TESTS = [
    (1, "Create", test_create, False),
    (2, "Read", test_read, True),
    (3, "Update", test_update, False),
    (4, "State moves", test_state_moves, False),
    (5, "Relations", test_relations, False),
    (6, "Duplicate merge", test_duplicate_merge, False),
    (7, "Split", test_split, False),
    (8, "Comments", test_comments, False),
    (9, "Lookups", test_lookups, True),
    (10, "Errors", test_errors, True),
    (11, "Sub-issues", test_sub_issues, False),
]
