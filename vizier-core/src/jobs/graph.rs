use super::*;

pub struct ScheduleGraph {
    records: HashMap<String, JobRecord>,
    after: HashMap<String, Vec<JobAfterDependency>>,
    after_dependents: HashMap<String, Vec<String>>,
    dependencies: HashMap<String, Vec<JobArtifact>>,
    artifacts: HashMap<String, Vec<JobArtifact>>,
    producers: HashMap<JobArtifact, Vec<String>>,
    consumers: HashMap<JobArtifact, Vec<String>>,
    job_order: Vec<String>,
}

impl ScheduleGraph {
    pub fn new(records: Vec<JobRecord>) -> Self {
        let mut records_map = HashMap::new();
        for record in records {
            records_map.insert(record.id.clone(), record);
        }

        let mut after = HashMap::new();
        let mut after_dependents: HashMap<String, Vec<String>> = HashMap::new();
        let mut dependencies = HashMap::new();
        let mut artifacts = HashMap::new();
        let mut producers: HashMap<JobArtifact, Vec<String>> = HashMap::new();
        let mut consumers: HashMap<JobArtifact, Vec<String>> = HashMap::new();

        for record in records_map.values() {
            let schedule = record.schedule.as_ref();
            let mut after_deps = schedule
                .map(|sched| sched.after.clone())
                .unwrap_or_default();
            sort_after_dependencies(&mut after_deps);
            after_deps.dedup();
            for dep in &after_deps {
                after_dependents
                    .entry(dep.job_id.clone())
                    .or_default()
                    .push(record.id.clone());
            }
            after.insert(record.id.clone(), after_deps);

            let mut deps = schedule
                .map(|sched| {
                    sched
                        .dependencies
                        .iter()
                        .map(|dep| dep.artifact.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            sort_artifacts(&mut deps);
            dependencies.insert(record.id.clone(), deps);

            let mut produced = schedule
                .map(|sched| sched.artifacts.clone())
                .unwrap_or_default();
            sort_artifacts(&mut produced);
            artifacts.insert(record.id.clone(), produced.clone());

            for artifact in &produced {
                producers
                    .entry(artifact.clone())
                    .or_default()
                    .push(record.id.clone());
            }

            if let Some(schedule) = schedule {
                for dep in &schedule.dependencies {
                    consumers
                        .entry(dep.artifact.clone())
                        .or_default()
                        .push(record.id.clone());
                }
            }
        }

        for list in producers.values_mut() {
            sort_job_ids(list, &records_map);
        }
        for list in consumers.values_mut() {
            sort_job_ids(list, &records_map);
        }
        for list in after_dependents.values_mut() {
            sort_job_ids(list, &records_map);
        }

        let mut job_order = records_map.keys().cloned().collect::<Vec<_>>();
        sort_job_ids(&mut job_order, &records_map);

        Self {
            records: records_map,
            after,
            after_dependents,
            dependencies,
            artifacts,
            producers,
            consumers,
            job_order,
        }
    }

    pub fn record(&self, job_id: &str) -> Option<&JobRecord> {
        self.records.get(job_id)
    }

    pub fn job_ids_sorted(&self) -> Vec<String> {
        self.job_order.clone()
    }

    pub fn dependencies_for(&self, job_id: &str) -> Vec<JobArtifact> {
        self.dependencies.get(job_id).cloned().unwrap_or_default()
    }

    pub fn after_for(&self, job_id: &str) -> Vec<JobAfterDependency> {
        self.after.get(job_id).cloned().unwrap_or_default()
    }

    pub fn after_dependents_for(&self, job_id: &str) -> Vec<String> {
        self.after_dependents
            .get(job_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn artifacts_for(&self, job_id: &str) -> Vec<JobArtifact> {
        self.artifacts.get(job_id).cloned().unwrap_or_default()
    }

    pub fn producers_for(&self, artifact: &JobArtifact) -> Vec<String> {
        self.producers.get(artifact).cloned().unwrap_or_default()
    }

    pub fn consumers_for(&self, artifact: &JobArtifact) -> Vec<String> {
        self.consumers.get(artifact).cloned().unwrap_or_default()
    }

    pub fn artifact_state(
        &self,
        repo: &Repository,
        artifact: &JobArtifact,
    ) -> ScheduleArtifactState {
        if artifact_exists(repo, artifact) {
            ScheduleArtifactState::Present
        } else {
            ScheduleArtifactState::Missing
        }
    }

    pub fn collect_focus_jobs(&self, focus: &str, max_depth: usize) -> HashSet<String> {
        if !self.records.contains_key(focus) {
            return HashSet::new();
        }

        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(focus.to_string());
        queue.push_back((focus.to_string(), 0usize));

        while let Some((job_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut neighbors = Vec::new();
            for dep in self.after_for(&job_id) {
                neighbors.push(dep.job_id);
            }
            neighbors.extend(self.after_dependents_for(&job_id));
            for dep in self.dependencies_for(&job_id) {
                neighbors.extend(self.producers_for(&dep));
            }
            for artifact in self.artifacts_for(&job_id) {
                neighbors.extend(self.consumers_for(&artifact));
            }
            neighbors.sort_by(|a, b| self.job_compare(a, b));
            neighbors.dedup();
            for neighbor in neighbors {
                if seen.insert(neighbor.clone()) {
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        seen
    }

    pub fn snapshot_edges(
        &self,
        repo: &Repository,
        roots: &[String],
        max_depth: usize,
    ) -> Vec<ScheduleEdge> {
        let mut edges = Vec::new();

        for root in roots {
            if !self.records.contains_key(root) {
                continue;
            }
            let mut path = HashSet::new();
            path.insert(root.clone());
            self.collect_edges(repo, root, max_depth, &mut path, &mut edges);
        }

        edges
    }

    fn collect_edges(
        &self,
        repo: &Repository,
        job_id: &str,
        depth_remaining: usize,
        path: &mut HashSet<String>,
        edges: &mut Vec<ScheduleEdge>,
    ) {
        if depth_remaining == 0 {
            return;
        }

        for after in self.after_for(job_id) {
            edges.push(ScheduleEdge {
                from: job_id.to_string(),
                to: after.job_id.clone(),
                artifact: None,
                state: None,
                after: Some(ScheduleAfterEdge {
                    policy: after.policy,
                }),
            });
            if depth_remaining > 1 && !path.contains(&after.job_id) {
                path.insert(after.job_id.clone());
                self.collect_edges(repo, &after.job_id, depth_remaining - 1, path, edges);
                path.remove(&after.job_id);
            }
        }

        for dependency in self.dependencies_for(job_id) {
            let artifact_label = format_artifact(&dependency);
            let producers = self.producers_for(&dependency);
            if producers.is_empty() {
                let state = self.artifact_state(repo, &dependency);
                edges.push(ScheduleEdge {
                    from: job_id.to_string(),
                    to: format!("artifact:{artifact_label}"),
                    artifact: Some(artifact_label),
                    state: Some(state),
                    after: None,
                });
                continue;
            }

            for producer_id in producers {
                edges.push(ScheduleEdge {
                    from: job_id.to_string(),
                    to: producer_id.clone(),
                    artifact: Some(artifact_label.clone()),
                    state: None,
                    after: None,
                });
                if depth_remaining > 1 && !path.contains(&producer_id) {
                    path.insert(producer_id.clone());
                    self.collect_edges(repo, &producer_id, depth_remaining - 1, path, edges);
                    path.remove(&producer_id);
                }
            }
        }
    }

    fn job_compare(&self, a: &str, b: &str) -> Ordering {
        match (self.records.get(a), self.records.get(b)) {
            (Some(left), Some(right)) => {
                let order = left.created_at.cmp(&right.created_at);
                if order == Ordering::Equal {
                    left.id.cmp(&right.id)
                } else {
                    order
                }
            }
            _ => a.cmp(b),
        }
    }
}

pub(crate) fn artifact_sort_key(artifact: &JobArtifact) -> (u8, &str, &str) {
    match artifact {
        JobArtifact::PlanBranch { slug, branch } => (0, slug, branch),
        JobArtifact::PlanDoc { slug, branch } => (1, slug, branch),
        JobArtifact::PlanCommits { slug, branch } => (2, slug, branch),
        JobArtifact::TargetBranch { name } => (3, name, ""),
        JobArtifact::MergeSentinel { slug } => (4, slug, ""),
        JobArtifact::CommandPatch { job_id } => (5, job_id, ""),
        JobArtifact::Custom { type_id, key } => (6, type_id, key),
    }
}

pub(crate) fn after_policy_sort_key(policy: AfterPolicy) -> u8 {
    match policy {
        AfterPolicy::Success => 0,
    }
}

pub(crate) fn sort_after_dependencies(dependencies: &mut [JobAfterDependency]) {
    dependencies.sort_by(|left, right| {
        let left_key = (left.job_id.as_str(), after_policy_sort_key(left.policy));
        let right_key = (right.job_id.as_str(), after_policy_sort_key(right.policy));
        left_key.cmp(&right_key)
    });
}

pub(crate) fn sort_artifacts(artifacts: &mut [JobArtifact]) {
    artifacts.sort_by(|left, right| artifact_sort_key(left).cmp(&artifact_sort_key(right)));
}

pub(crate) fn sort_job_ids(job_ids: &mut [String], records: &HashMap<String, JobRecord>) {
    job_ids.sort_by(
        |left, right| match (records.get(left), records.get(right)) {
            (Some(left_record), Some(right_record)) => {
                let order = left_record.created_at.cmp(&right_record.created_at);
                if order == Ordering::Equal {
                    left_record.id.cmp(&right_record.id)
                } else {
                    order
                }
            }
            _ => left.cmp(right),
        },
    );
}
