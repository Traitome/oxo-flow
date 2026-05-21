use oxo_flow_core::cluster::{generate_submit_script_with_env, ClusterBackend, ClusterJobConfig};
use oxo_flow_core::rule::{Rule, EnvironmentSpec, Resources};
use oxo_flow_core::environment::EnvironmentResolver;
use std::collections::HashMap;

fn main() {
    let rule = Rule {
        name: "gatk_job".to_string(),
        input: vec![].into(),
        output: vec![].into(),
        shell: Some("gatk HaplotypeCaller".to_string()),
        script: None,
        threads: Some(4),
        memory: Some("8G".to_string()),
        resources: Resources::default(),
        environment: EnvironmentSpec {
            modules: vec!["java/11".to_string(), "gatk/4.2".to_string()],
            ..EnvironmentSpec::default()
        },
        log: None,
        benchmark: None,
        params: HashMap::new(),
        priority: 0,
        target: false,
        group: None,
        description: None,
        ..Default::default()
    };
    let config = ClusterJobConfig {
        backend: ClusterBackend::Slurm,
        queue: Some("compute".to_string()),
        account: Some("proj123".to_string()),
        walltime: Some("24:00:00".to_string()),
        extra_args: vec![],
    };
    let env_resolver = EnvironmentResolver::new();
    let script = generate_submit_script_with_env(
        &ClusterBackend::Slurm,
        &rule,
        "gatk HaplotypeCaller",
        &config,
        &env_resolver,
    ).unwrap();
    
    println!("Generated script:\n{}", script);
    println!("\nContains 'module load': {}", script.contains("module load"));
}
