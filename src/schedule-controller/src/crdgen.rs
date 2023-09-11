use kube::CustomResourceExt;
use schedule_controller::controller::SchedulePolicy;

fn main() {
    print!("{}", serde_yaml::to_string(&SchedulePolicy::crd()).unwrap());
}
