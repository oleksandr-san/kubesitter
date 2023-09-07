use kube::{CustomResourceExt, Resource};
fn main() {
    print!("{}", controller::SchedulePolicy::api_version(&()));
    print!(
        "{}",
        serde_yaml::to_string(&controller::SchedulePolicy::crd()).unwrap()
    )
}
