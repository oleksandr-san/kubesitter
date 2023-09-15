use kube::CustomResourceExt;
use kubesitter::model::SchedulePolicy;

fn main() {
    print!("{}", serde_yaml::to_string(&SchedulePolicy::crd()).unwrap());
}
