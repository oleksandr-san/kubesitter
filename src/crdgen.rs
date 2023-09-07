use kube::CustomResourceExt;
fn main() {
    print!(
        "{}",
        serde_yaml::to_string(&controller::SchedulePolicy::crd()).unwrap()
    )
}
