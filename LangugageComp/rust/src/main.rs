use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::process;

const HELP: &str = "BuildGraph - deterministic dependency graph analyzer\n\n\
usage:\n\
\x20\x20buildgraph analyze <file>\n\
\x20\x20buildgraph affected <file> <task>\n\
\x20\x20buildgraph --help\n";
const USAGE_ERROR: &str = "expected 'analyze <file>' or 'affected <file> <task>'";

#[derive(Debug)]
struct Task {
    name: String,
    duration: i64,
    line: usize,
}

#[derive(Debug)]
struct Graph {
    tasks: Vec<Task>,
    edge_from: Vec<usize>,
    edge_to: Vec<usize>,
    indegree: Vec<usize>,
    indexes: HashMap<String, usize>,
}

fn is_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 32 || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
}

fn line_error(line: usize, message: &str) -> String {
    format!("line {line}: {message}")
}

fn parse_graph(text: &str) -> Result<Graph, String> {
    let mut tasks = Vec::new();
    let mut dependency_specs = Vec::new();
    let mut indexes = HashMap::new();

    for (zero_based_line, raw_line) in text.split('\n').enumerate() {
        let line_number = zero_based_line + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() != 3 {
            return Err(line_error(
                line_number,
                "expected exactly three '|' separated fields",
            ));
        }

        let name = fields[0].trim();
        let duration_text = fields[1].trim();
        let dependency_spec = fields[2].trim();

        if !is_identifier(name) {
            return Err(line_error(
                line_number,
                &format!("invalid task identifier '{name}'"),
            ));
        }
        if duration_text.is_empty() || !duration_text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(line_error(
                line_number,
                &format!("invalid duration '{duration_text}'"),
            ));
        }
        let duration = duration_text
            .parse::<i64>()
            .map_err(|_| line_error(line_number, &format!("invalid duration '{duration_text}'")))?;
        if !(1..=i32::MAX as i64).contains(&duration) {
            return Err(line_error(
                line_number,
                &format!("invalid duration '{duration_text}'"),
            ));
        }
        if indexes.contains_key(name) {
            return Err(line_error(line_number, &format!("duplicate task '{name}'")));
        }

        let index = tasks.len();
        indexes.insert(name.to_owned(), index);
        tasks.push(Task {
            name: name.to_owned(),
            duration,
            line: line_number,
        });
        dependency_specs.push(dependency_spec.to_owned());
    }

    if tasks.is_empty() {
        return Err("no tasks".to_owned());
    }

    let mut edge_from = Vec::new();
    let mut edge_to = Vec::new();
    let mut indegree = vec![0; tasks.len()];

    for (task_index, dependency_spec) in dependency_specs.iter().enumerate() {
        if dependency_spec.is_empty() {
            continue;
        }

        let task = &tasks[task_index];
        let mut seen = HashSet::new();
        for raw_dependency in dependency_spec.split(',') {
            let dependency = raw_dependency.trim();
            if dependency.is_empty() {
                return Err(line_error(
                    task.line,
                    &format!("empty dependency for task '{}'", task.name),
                ));
            }
            if !is_identifier(dependency) {
                return Err(line_error(
                    task.line,
                    &format!("invalid dependency identifier '{dependency}'"),
                ));
            }
            if dependency == task.name {
                return Err(line_error(
                    task.line,
                    &format!("task '{}' depends on itself", task.name),
                ));
            }
            if !seen.insert(dependency.to_owned()) {
                return Err(line_error(
                    task.line,
                    &format!(
                        "duplicate dependency '{dependency}' for task '{}'",
                        task.name
                    ),
                ));
            }
            let Some(&dependency_index) = indexes.get(dependency) else {
                return Err(line_error(
                    task.line,
                    &format!("unknown dependency '{dependency}' for task '{}'", task.name),
                ));
            };
            edge_from.push(dependency_index);
            edge_to.push(task_index);
            indegree[task_index] += 1;
        }
    }

    Ok(Graph {
        tasks,
        edge_from,
        edge_to,
        indegree,
        indexes,
    })
}

fn stable_topological_order(graph: &Graph) -> Result<Vec<usize>, String> {
    let count = graph.tasks.len();
    let mut indegree = graph.indegree.clone();
    let mut processed = vec![false; count];
    let mut order = Vec::with_capacity(count);

    while order.len() < count {
        let next = (0..count).find(|&index| !processed[index] && indegree[index] == 0);
        let Some(selected) = next else {
            return Err("cycle detected".to_owned());
        };

        processed[selected] = true;
        order.push(selected);
        for edge in 0..graph.edge_from.len() {
            if graph.edge_from[edge] == selected {
                indegree[graph.edge_to[edge]] -= 1;
            }
        }
    }

    Ok(order)
}

fn build_path(previous: &[Option<usize>], end: usize) -> Vec<usize> {
    let mut reversed = Vec::new();
    let mut current = Some(end);
    while let Some(index) = current {
        reversed.push(index);
        current = previous[index];
    }
    reversed.reverse();
    reversed
}

fn candidate_path_is_earlier(
    previous: &[Option<usize>],
    candidate_end: usize,
    current_end: Option<usize>,
) -> bool {
    let Some(current_end) = current_end else {
        return true;
    };
    build_path(previous, candidate_end) < build_path(previous, current_end)
}

fn critical_path(graph: &Graph, order: &[usize]) -> Result<(i64, Vec<usize>), String> {
    let mut distance = vec![0_i64; graph.tasks.len()];
    let mut previous = vec![None; graph.tasks.len()];

    for &task_index in order {
        let mut best_distance = graph.tasks[task_index].duration;
        let mut best_previous = None;

        for edge in 0..graph.edge_from.len() {
            if graph.edge_to[edge] != task_index {
                continue;
            }
            let dependency = graph.edge_from[edge];
            let candidate = distance[dependency]
                .checked_add(graph.tasks[task_index].duration)
                .ok_or_else(|| "critical duration overflow".to_owned())?;
            if candidate > best_distance
                || (candidate == best_distance
                    && candidate_path_is_earlier(&previous, dependency, best_previous))
            {
                best_distance = candidate;
                best_previous = Some(dependency);
            }
        }

        distance[task_index] = best_distance;
        previous[task_index] = best_previous;
    }

    let mut best_end = order[0];
    for &task_index in &order[1..] {
        if distance[task_index] > distance[best_end]
            || (distance[task_index] == distance[best_end]
                && build_path(&previous, task_index) < build_path(&previous, best_end))
        {
            best_end = task_index;
        }
    }

    Ok((distance[best_end], build_path(&previous, best_end)))
}

fn analyze(graph: &Graph) -> Result<String, String> {
    let order = stable_topological_order(graph)?;
    let (duration, path) = critical_path(graph, &order)?;
    let order_names: Vec<&str> = order
        .iter()
        .map(|&index| graph.tasks[index].name.as_str())
        .collect();
    let path_names: Vec<&str> = path
        .iter()
        .map(|&index| graph.tasks[index].name.as_str())
        .collect();

    Ok(format!(
        "tasks: {}\norder: {}\ncritical-duration: {}\ncritical-path: {}\n",
        graph.tasks.len(),
        order_names.join(", "),
        duration,
        path_names.join(" -> ")
    ))
}

fn affected(graph: &Graph, task_name: &str) -> Result<String, String> {
    let Some(&query) = graph.indexes.get(task_name) else {
        return Err(format!("unknown task '{task_name}'"));
    };
    let order = stable_topological_order(graph)?;
    let mut marked = vec![false; graph.tasks.len()];

    for &task_index in &order {
        let mut is_affected = task_index == query;
        if !is_affected {
            for edge in 0..graph.edge_from.len() {
                if graph.edge_to[edge] == task_index && marked[graph.edge_from[edge]] {
                    is_affected = true;
                    break;
                }
            }
        }
        marked[task_index] = is_affected;
    }

    let names: Vec<&str> = order
        .iter()
        .copied()
        .filter(|&index| marked[index])
        .map(|index| graph.tasks[index].name.as_str())
        .collect();
    Ok(format!("affected: {}\n", names.join(", ")))
}

fn fail(prefix: &str, message: &str, code: i32) -> ! {
    eprintln!("{prefix}: {message}");
    process::exit(code);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        print!("{HELP}");
        return;
    }

    let (command, path, query) = match args.as_slice() {
        [command, path] if command == "analyze" => (command.as_str(), path.as_str(), None),
        [command, path, query] if command == "affected" => {
            (command.as_str(), path.as_str(), Some(query.as_str()))
        }
        _ => fail("usage error", USAGE_ERROR, 2),
    };

    let text = fs::read_to_string(path)
        .unwrap_or_else(|_| fail("io error", &format!("unable to read '{path}'"), 3));
    let graph = parse_graph(&text).unwrap_or_else(|message| fail("input error", &message, 4));
    let output = if command == "analyze" {
        analyze(&graph)
    } else {
        affected(&graph, query.expect("affected command has a task"))
    }
    .unwrap_or_else(|message| fail("input error", &message, 4));
    print!("{output}");
}
