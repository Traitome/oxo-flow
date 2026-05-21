#[test]
fn test_modules_wrap() {
    use oxo_flow_core::environment::EnvironmentResolver;
    use oxo_flow_core::rule::EnvironmentSpec;
    
    let env_resolver = EnvironmentResolver::new();
    let env_spec = EnvironmentSpec {
        modules: vec!["java/11".to_string(), "gatk/4.2".to_string()],
        ..EnvironmentSpec::default()
    };
    
    let result = env_resolver.wrap_command("echo test", &env_spec, None);
    println!("Result: {:?}", result);
    
    match result {
        Ok(cmd) => {
            println!("Command: {}", cmd);
            assert!(cmd.contains("module load"));
        }
        Err(e) => {
            println!("Error: {}", e);
            panic!("wrap_command failed: {}", e);
        }
    }
}

fn main() {
    test_modules_wrap();
    println!("Test passed!");
}
