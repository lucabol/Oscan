declare const require: (name: string) => unknown;
declare const process: {
  argv: string[];
  stdout: { write(value: string): void };
  stderr: { write(value: string): void };
  exit(code: number): never;
};

const fs = require("node:fs") as {
  readFileSync(path: string, encoding: "utf8"): string;
};

const HELP =
  "BuildGraph - deterministic dependency graph analyzer\n\n" +
  "usage:\n" +
  "  buildgraph analyze <file>\n" +
  "  buildgraph affected <file> <task>\n" +
  "  buildgraph --help\n";
const USAGE_ERROR =
  "expected 'analyze <file>' or 'affected <file> <task>'";

interface TaskSpec {
  readonly name: string;
  readonly duration: bigint;
  readonly line: number;
}

interface Graph {
  readonly tasks: TaskSpec[];
  readonly edgeFrom: number[];
  readonly edgeTo: number[];
  readonly indegree: number[];
  readonly indexes: Map<string, number>;
}

class InputError extends Error {}

function isAsciiAlpha(code: number): boolean {
  return (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
}

function isAsciiDigit(code: number): boolean {
  return code >= 48 && code <= 57;
}

function isIdentifier(value: string): boolean {
  if (value.length === 0 || value.length > 32 || !isAsciiAlpha(value.charCodeAt(0))) {
    return false;
  }
  for (let index = 1; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (!isAsciiAlpha(code) && !isAsciiDigit(code) && code !== 95 && code !== 45) {
      return false;
    }
  }
  return true;
}

function lineError(line: number, message: string): InputError {
  return new InputError(`line ${line}: ${message}`);
}

function parseGraph(text: string): Graph {
  const tasks: TaskSpec[] = [];
  const dependencySpecs: string[] = [];
  const indexes = new Map<string, number>();
  const lines = text.split("\n");

  for (let zeroBasedLine = 0; zeroBasedLine < lines.length; zeroBasedLine += 1) {
    const line = lines[zeroBasedLine]!.trim();
    const lineNumber = zeroBasedLine + 1;
    if (line.length === 0 || line.startsWith("#")) {
      continue;
    }

    const fields = line.split("|");
    if (fields.length !== 3) {
      throw lineError(lineNumber, "expected exactly three '|' separated fields");
    }

    const name = fields[0]!.trim();
    const durationText = fields[1]!.trim();
    const dependencySpec = fields[2]!.trim();

    if (!isIdentifier(name)) {
      throw lineError(lineNumber, `invalid task identifier '${name}'`);
    }
    if (
      durationText.length === 0 ||
      [...durationText].some((character) => !isAsciiDigit(character.charCodeAt(0)))
    ) {
      throw lineError(lineNumber, `invalid duration '${durationText}'`);
    }

    let duration: bigint;
    try {
      duration = BigInt(durationText);
    } catch {
      throw lineError(lineNumber, `invalid duration '${durationText}'`);
    }
    if (duration < 1n || duration > 2147483647n) {
      throw lineError(lineNumber, `invalid duration '${durationText}'`);
    }
    if (indexes.has(name)) {
      throw lineError(lineNumber, `duplicate task '${name}'`);
    }

    indexes.set(name, tasks.length);
    tasks.push({ name, duration, line: lineNumber });
    dependencySpecs.push(dependencySpec);
  }

  if (tasks.length === 0) {
    throw new InputError("no tasks");
  }

  const edgeFrom: number[] = [];
  const edgeTo: number[] = [];
  const indegree = tasks.map(() => 0);

  for (let taskIndex = 0; taskIndex < tasks.length; taskIndex += 1) {
    const dependencySpec = dependencySpecs[taskIndex]!;
    if (dependencySpec.length === 0) {
      continue;
    }

    const task = tasks[taskIndex]!;
    const seen = new Set<string>();
    for (const rawDependency of dependencySpec.split(",")) {
      const dependency = rawDependency.trim();
      if (dependency.length === 0) {
        throw lineError(task.line, `empty dependency for task '${task.name}'`);
      }
      if (!isIdentifier(dependency)) {
        throw lineError(task.line, `invalid dependency identifier '${dependency}'`);
      }
      if (dependency === task.name) {
        throw lineError(task.line, `task '${task.name}' depends on itself`);
      }
      if (seen.has(dependency)) {
        throw lineError(
          task.line,
          `duplicate dependency '${dependency}' for task '${task.name}'`,
        );
      }
      seen.add(dependency);
      const dependencyIndex = indexes.get(dependency);
      if (dependencyIndex === undefined) {
        throw lineError(
          task.line,
          `unknown dependency '${dependency}' for task '${task.name}'`,
        );
      }
      edgeFrom.push(dependencyIndex);
      edgeTo.push(taskIndex);
      indegree[taskIndex] = indegree[taskIndex]! + 1;
    }
  }

  return { tasks, edgeFrom, edgeTo, indegree, indexes };
}

function stableTopologicalOrder(graph: Graph): number[] {
  const indegree = [...graph.indegree];
  const processed = graph.tasks.map(() => false);
  const order: number[] = [];

  while (order.length < graph.tasks.length) {
    let selected = -1;
    for (let index = 0; index < graph.tasks.length; index += 1) {
      if (!processed[index] && indegree[index] === 0) {
        selected = index;
        break;
      }
    }
    if (selected < 0) {
      throw new InputError("cycle detected");
    }

    processed[selected] = true;
    order.push(selected);
    for (let edge = 0; edge < graph.edgeFrom.length; edge += 1) {
      if (graph.edgeFrom[edge] === selected) {
        const dependent = graph.edgeTo[edge]!;
        indegree[dependent] = indegree[dependent]! - 1;
      }
    }
  }

  return order;
}

function buildPath(previous: Array<number | undefined>, end: number): number[] {
  const reversed: number[] = [];
  let current: number | undefined = end;
  while (current !== undefined) {
    reversed.push(current);
    current = previous[current];
  }
  reversed.reverse();
  return reversed;
}

function comparePaths(left: number[], right: number[]): number {
  const common = Math.min(left.length, right.length);
  for (let index = 0; index < common; index += 1) {
    const difference = left[index]! - right[index]!;
    if (difference !== 0) {
      return difference;
    }
  }
  return left.length - right.length;
}

function candidatePathIsEarlier(
  previous: Array<number | undefined>,
  candidateEnd: number,
  currentEnd: number | undefined,
): boolean {
  return (
    currentEnd === undefined ||
    comparePaths(buildPath(previous, candidateEnd), buildPath(previous, currentEnd)) < 0
  );
}

function criticalPath(graph: Graph, order: number[]): [bigint, number[]] {
  const distance = graph.tasks.map(() => 0n);
  const previous: Array<number | undefined> = graph.tasks.map(() => undefined);

  for (const taskIndex of order) {
    let bestDistance = graph.tasks[taskIndex]!.duration;
    let bestPrevious: number | undefined;

    for (let edge = 0; edge < graph.edgeFrom.length; edge += 1) {
      if (graph.edgeTo[edge] !== taskIndex) {
        continue;
      }
      const dependency = graph.edgeFrom[edge]!;
      const candidate = distance[dependency]! + graph.tasks[taskIndex]!.duration;
      if (candidate > 9223372036854775807n) {
        throw new InputError("critical duration overflow");
      }
      if (
        candidate > bestDistance ||
        (candidate === bestDistance &&
          candidatePathIsEarlier(previous, dependency, bestPrevious))
      ) {
        bestDistance = candidate;
        bestPrevious = dependency;
      }
    }

    distance[taskIndex] = bestDistance;
    previous[taskIndex] = bestPrevious;
  }

  let bestEnd = order[0]!;
  for (let position = 1; position < order.length; position += 1) {
    const taskIndex = order[position]!;
    if (
      distance[taskIndex]! > distance[bestEnd]! ||
      (distance[taskIndex] === distance[bestEnd] &&
        comparePaths(buildPath(previous, taskIndex), buildPath(previous, bestEnd)) < 0)
    ) {
      bestEnd = taskIndex;
    }
  }

  return [distance[bestEnd]!, buildPath(previous, bestEnd)];
}

function analyze(graph: Graph): string {
  const order = stableTopologicalOrder(graph);
  const [duration, path] = criticalPath(graph, order);
  return (
    `tasks: ${graph.tasks.length}\n` +
    `order: ${order.map((index) => graph.tasks[index]!.name).join(", ")}\n` +
    `critical-duration: ${duration.toString()}\n` +
    `critical-path: ${path.map((index) => graph.tasks[index]!.name).join(" -> ")}\n`
  );
}

function affected(graph: Graph, taskName: string): string {
  const query = graph.indexes.get(taskName);
  if (query === undefined) {
    throw new InputError(`unknown task '${taskName}'`);
  }
  const order = stableTopologicalOrder(graph);
  const marked = graph.tasks.map(() => false);

  for (const taskIndex of order) {
    let isAffected = taskIndex === query;
    if (!isAffected) {
      for (let edge = 0; edge < graph.edgeFrom.length; edge += 1) {
        if (graph.edgeTo[edge] === taskIndex && marked[graph.edgeFrom[edge]!] === true) {
          isAffected = true;
          break;
        }
      }
    }
    marked[taskIndex] = isAffected;
  }

  return `affected: ${order
    .filter((index) => marked[index])
    .map((index) => graph.tasks[index]!.name)
    .join(", ")}\n`;
}

function fail(prefix: string, message: string, code: number): never {
  process.stderr.write(`${prefix}: ${message}\n`);
  process.exit(code);
}

function main(): void {
  const args = process.argv.slice(2);
  if (args.length === 1 && (args[0] === "--help" || args[0] === "-h")) {
    process.stdout.write(HELP);
    return;
  }

  let command: "analyze" | "affected";
  let path: string;
  let query: string | undefined;
  if (args.length === 2 && args[0] === "analyze") {
    command = "analyze";
    path = args[1]!;
  } else if (args.length === 3 && args[0] === "affected") {
    command = "affected";
    path = args[1]!;
    query = args[2]!;
  } else {
    fail("usage error", USAGE_ERROR, 2);
  }

  let text: string;
  try {
    text = fs.readFileSync(path, "utf8");
  } catch {
    fail("io error", `unable to read '${path}'`, 3);
  }

  try {
    const graph = parseGraph(text);
    process.stdout.write(
      command === "analyze" ? analyze(graph) : affected(graph, query!),
    );
  } catch (error: unknown) {
    if (error instanceof InputError) {
      fail("input error", error.message, 4);
    }
    throw error;
  }
}

main();

