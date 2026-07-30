using System.Globalization;
using System.Text;

const string Help =
    "BuildGraph - deterministic dependency graph analyzer\n\n" +
    "usage:\n" +
    "  buildgraph analyze <file>\n" +
    "  buildgraph affected <file> <task>\n" +
    "  buildgraph --help\n";
const string UsageError =
    "expected 'analyze <file>' or 'affected <file> <task>'";

return ProgramMain(args);

static int ProgramMain(string[] args)
{
    if (args.Length == 1 && (args[0] == "--help" || args[0] == "-h"))
    {
        Console.Out.Write(Help);
        return 0;
    }

    string command;
    string path;
    string? query = null;
    if (args.Length == 2 && args[0] == "analyze")
    {
        command = "analyze";
        path = args[1];
    }
    else if (args.Length == 3 && args[0] == "affected")
    {
        command = "affected";
        path = args[1];
        query = args[2];
    }
    else
    {
        return Fail("usage error", UsageError, 2);
    }

    string text;
    try
    {
        text = File.ReadAllText(path, Encoding.UTF8);
    }
    catch (Exception exception) when (
        exception is IOException or UnauthorizedAccessException or ArgumentException)
    {
        return Fail("io error", $"unable to read '{path}'", 3);
    }

    try
    {
        Graph graph = ParseGraph(text);
        Console.Out.Write(command == "analyze"
            ? Analyze(graph)
            : Affected(graph, query!));
        return 0;
    }
    catch (InputException exception)
    {
        return Fail("input error", exception.Message, 4);
    }
}

static int Fail(string prefix, string message, int code)
{
    Console.Error.Write($"{prefix}: {message}\n");
    return code;
}

static bool IsAsciiAlpha(char value) =>
    value is >= 'A' and <= 'Z' or >= 'a' and <= 'z';

static bool IsAsciiDigit(char value) => value is >= '0' and <= '9';

static bool IsIdentifier(string value)
{
    if (value.Length is 0 or > 32 || !IsAsciiAlpha(value[0]))
    {
        return false;
    }
    for (int index = 1; index < value.Length; index++)
    {
        char character = value[index];
        if (!IsAsciiAlpha(character) &&
            !IsAsciiDigit(character) &&
            character != '_' &&
            character != '-')
        {
            return false;
        }
    }
    return true;
}

static InputException LineError(int line, string message) =>
    new($"line {line}: {message}");

static Graph ParseGraph(string text)
{
    List<TaskSpec> tasks = [];
    List<string> dependencySpecs = [];
    Dictionary<string, int> indexes = new(StringComparer.Ordinal);
    string[] lines = text.Split('\n');

    for (int zeroBasedLine = 0; zeroBasedLine < lines.Length; zeroBasedLine++)
    {
        int lineNumber = zeroBasedLine + 1;
        string line = lines[zeroBasedLine].Trim();
        if (line.Length == 0 || line.StartsWith('#'))
        {
            continue;
        }

        string[] fields = line.Split('|');
        if (fields.Length != 3)
        {
            throw LineError(lineNumber, "expected exactly three '|' separated fields");
        }

        string name = fields[0].Trim();
        string durationText = fields[1].Trim();
        string dependencySpec = fields[2].Trim();

        if (!IsIdentifier(name))
        {
            throw LineError(lineNumber, $"invalid task identifier '{name}'");
        }
        if (durationText.Length == 0 || durationText.Any(character => !IsAsciiDigit(character)))
        {
            throw LineError(lineNumber, $"invalid duration '{durationText}'");
        }
        if (!long.TryParse(
                durationText,
                NumberStyles.None,
                CultureInfo.InvariantCulture,
                out long duration) ||
            duration is < 1 or > int.MaxValue)
        {
            throw LineError(lineNumber, $"invalid duration '{durationText}'");
        }
        if (indexes.ContainsKey(name))
        {
            throw LineError(lineNumber, $"duplicate task '{name}'");
        }

        indexes.Add(name, tasks.Count);
        tasks.Add(new TaskSpec(name, duration, lineNumber));
        dependencySpecs.Add(dependencySpec);
    }

    if (tasks.Count == 0)
    {
        throw new InputException("no tasks");
    }

    List<int> edgeFrom = [];
    List<int> edgeTo = [];
    int[] indegree = new int[tasks.Count];

    for (int taskIndex = 0; taskIndex < tasks.Count; taskIndex++)
    {
        string dependencySpec = dependencySpecs[taskIndex];
        if (dependencySpec.Length == 0)
        {
            continue;
        }

        TaskSpec task = tasks[taskIndex];
        HashSet<string> seen = new(StringComparer.Ordinal);
        foreach (string rawDependency in dependencySpec.Split(','))
        {
            string dependency = rawDependency.Trim();
            if (dependency.Length == 0)
            {
                throw LineError(task.Line, $"empty dependency for task '{task.Name}'");
            }
            if (!IsIdentifier(dependency))
            {
                throw LineError(task.Line, $"invalid dependency identifier '{dependency}'");
            }
            if (dependency == task.Name)
            {
                throw LineError(task.Line, $"task '{task.Name}' depends on itself");
            }
            if (!seen.Add(dependency))
            {
                throw LineError(
                    task.Line,
                    $"duplicate dependency '{dependency}' for task '{task.Name}'");
            }
            if (!indexes.TryGetValue(dependency, out int dependencyIndex))
            {
                throw LineError(
                    task.Line,
                    $"unknown dependency '{dependency}' for task '{task.Name}'");
            }
            edgeFrom.Add(dependencyIndex);
            edgeTo.Add(taskIndex);
            indegree[taskIndex]++;
        }
    }

    return new Graph(tasks, edgeFrom, edgeTo, indegree, indexes);
}

static List<int> StableTopologicalOrder(Graph graph)
{
    int[] indegree = (int[])graph.Indegree.Clone();
    bool[] processed = new bool[graph.Tasks.Count];
    List<int> order = new(graph.Tasks.Count);

    while (order.Count < graph.Tasks.Count)
    {
        int selected = -1;
        for (int index = 0; index < graph.Tasks.Count; index++)
        {
            if (!processed[index] && indegree[index] == 0)
            {
                selected = index;
                break;
            }
        }
        if (selected < 0)
        {
            throw new InputException("cycle detected");
        }

        processed[selected] = true;
        order.Add(selected);
        for (int edge = 0; edge < graph.EdgeFrom.Count; edge++)
        {
            if (graph.EdgeFrom[edge] == selected)
            {
                indegree[graph.EdgeTo[edge]]--;
            }
        }
    }

    return order;
}

static List<int> BuildPath(int?[] previous, int end)
{
    List<int> reversed = [];
    int? current = end;
    while (current is int index)
    {
        reversed.Add(index);
        current = previous[index];
    }
    reversed.Reverse();
    return reversed;
}

static int ComparePaths(IReadOnlyList<int> left, IReadOnlyList<int> right)
{
    int common = Math.Min(left.Count, right.Count);
    for (int index = 0; index < common; index++)
    {
        int comparison = left[index].CompareTo(right[index]);
        if (comparison != 0)
        {
            return comparison;
        }
    }
    return left.Count.CompareTo(right.Count);
}

static bool CandidatePathIsEarlier(int?[] previous, int candidateEnd, int? currentEnd) =>
    currentEnd is null ||
    ComparePaths(BuildPath(previous, candidateEnd), BuildPath(previous, currentEnd.Value)) < 0;

static (long Duration, List<int> Path) CriticalPath(Graph graph, IReadOnlyList<int> order)
{
    long[] distance = new long[graph.Tasks.Count];
    int?[] previous = new int?[graph.Tasks.Count];

    foreach (int taskIndex in order)
    {
        long bestDistance = graph.Tasks[taskIndex].Duration;
        int? bestPrevious = null;

        for (int edge = 0; edge < graph.EdgeFrom.Count; edge++)
        {
            if (graph.EdgeTo[edge] != taskIndex)
            {
                continue;
            }
            int dependency = graph.EdgeFrom[edge];
            long candidate;
            try
            {
                candidate = checked(distance[dependency] + graph.Tasks[taskIndex].Duration);
            }
            catch (OverflowException)
            {
                throw new InputException("critical duration overflow");
            }

            if (candidate > bestDistance ||
                (candidate == bestDistance &&
                 CandidatePathIsEarlier(previous, dependency, bestPrevious)))
            {
                bestDistance = candidate;
                bestPrevious = dependency;
            }
        }

        distance[taskIndex] = bestDistance;
        previous[taskIndex] = bestPrevious;
    }

    int bestEnd = order[0];
    for (int position = 1; position < order.Count; position++)
    {
        int taskIndex = order[position];
        if (distance[taskIndex] > distance[bestEnd] ||
            (distance[taskIndex] == distance[bestEnd] &&
             ComparePaths(BuildPath(previous, taskIndex), BuildPath(previous, bestEnd)) < 0))
        {
            bestEnd = taskIndex;
        }
    }

    return (distance[bestEnd], BuildPath(previous, bestEnd));
}

static string Analyze(Graph graph)
{
    List<int> order = StableTopologicalOrder(graph);
    (long duration, List<int> path) = CriticalPath(graph, order);
    return
        $"tasks: {graph.Tasks.Count}\n" +
        $"order: {string.Join(", ", order.Select(index => graph.Tasks[index].Name))}\n" +
        $"critical-duration: {duration.ToString(CultureInfo.InvariantCulture)}\n" +
        $"critical-path: {string.Join(" -> ", path.Select(index => graph.Tasks[index].Name))}\n";
}

static string Affected(Graph graph, string taskName)
{
    if (!graph.Indexes.TryGetValue(taskName, out int query))
    {
        throw new InputException($"unknown task '{taskName}'");
    }
    List<int> order = StableTopologicalOrder(graph);
    bool[] marked = new bool[graph.Tasks.Count];

    foreach (int taskIndex in order)
    {
        bool isAffected = taskIndex == query;
        if (!isAffected)
        {
            for (int edge = 0; edge < graph.EdgeFrom.Count; edge++)
            {
                if (graph.EdgeTo[edge] == taskIndex && marked[graph.EdgeFrom[edge]])
                {
                    isAffected = true;
                    break;
                }
            }
        }
        marked[taskIndex] = isAffected;
    }

    return
        $"affected: {string.Join(", ", order.Where(index => marked[index]).Select(index => graph.Tasks[index].Name))}\n";
}

sealed record TaskSpec(string Name, long Duration, int Line);

sealed record Graph(
    List<TaskSpec> Tasks,
    List<int> EdgeFrom,
    List<int> EdgeTo,
    int[] Indegree,
    Dictionary<string, int> Indexes);

sealed class InputException(string message) : Exception(message);
