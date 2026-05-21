// 快速测试modules处理
fn main() {
    let modules = vec!["java/11".to_string(), "gatk/4.2".to_string()];
    let spec = modules.join(",");
    let command = "gatk HaplotypeCaller";
    let result = format!("module load {} && {}", spec.replace(',', " "), command);
    println!("Result: {}", result);
    println!("Contains 'module load java/11': {}", result.contains("module load java/11"));
}
