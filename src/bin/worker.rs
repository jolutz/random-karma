use random_karma::worker_agent::KarmaTask;
use yew_agent::Registrable;

fn main() {
    // Set the panic hook to log detailed errors to the console
    console_error_panic_hook::set_once();
    KarmaTask::registrar().register();
}
