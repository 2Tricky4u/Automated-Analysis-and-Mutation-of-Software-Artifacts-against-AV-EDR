#include <iostream>
#include <string>
#include <fstream>
#include <chrono>
#include <thread>

#ifdef HAS_KRABSETW
#include <krabs.hpp>
#endif

class ETWConsumer {
public:
    ETWConsumer(const std::string& output_file) 
        : output_file_(output_file), running_(false) {}
    
    void start() {
        running_ = true;
        std::cout << "ETW Consumer starting, output: " << output_file_ << std::endl;
        
#ifdef HAS_KRABSETW
        // Configure krabsetw session
        krabs::user_trace trace(L"EDR-Lab-Session");
        
        // Process provider
        krabs::provider<> process_provider(L"Microsoft-Windows-Kernel-Process");
        process_provider.any(0x10); // WINEVENT_KEYWORD_PROCESS
        
        process_provider.add_on_event_callback([this](const EVENT_RECORD& record, const krabs::trace_context& context) {
            krabs::schema schema(record, context.schema_locator);
            krabs::parser parser(schema);
            
            try {
                auto process_id = parser.parse<uint32_t>(L"ProcessId");
                auto image_name = parser.parse<std::wstring>(L"ImageFileName");
                
                this->write_event("process", process_id, image_name);
            } catch (...) {
                // Event parsing failed, skip
            }
        });
        
        trace.enable(process_provider);
        
        // Network provider
        krabs::provider<> network_provider(L"Microsoft-Windows-TCPIP");
        network_provider.any(0x10);
        
        network_provider.add_on_event_callback([this](const EVENT_RECORD& record, const krabs::trace_context& context) {
            this->write_event("network", 0, L"TCP/IP Event");
        });
        
        trace.enable(network_provider);
        
        // Start trace
        trace.start();
#else
        // Simulation mode without krabsetw
        std::cout << "Running in simulation mode (krabsetw not available)" << std::endl;
        simulate_events();
#endif
    }
    
    void stop() {
        running_ = false;
        std::cout << "ETW Consumer stopping" << std::endl;
    }

private:
    void write_event(const std::string& type, uint32_t pid, const std::wstring& data) {
        std::ofstream out(output_file_, std::ios::app);
        if (out.is_open()) {
            auto now = std::chrono::system_clock::now().time_since_epoch().count();
            out << now << "," << type << "," << pid << "," 
                << std::string(data.begin(), data.end()) << std::endl;
        }
    }
    
    void simulate_events() {
        // Simulate events for testing when krabsetw is not available
        std::ofstream out(output_file_);
        out << "timestamp,event_type,process_id,details" << std::endl;
        
        int counter = 0;
        while (running_ && counter < 10) {
            auto now = std::chrono::system_clock::now().time_since_epoch().count();
            out << now << ",process," << (1000 + counter) << ",notepad.exe" << std::endl;
            
            std::this_thread::sleep_for(std::chrono::milliseconds(500));
            counter++;
        }
        out.close();
    }
    
    std::string output_file_;
    bool running_;
};

int main(int argc, char* argv[]) {
    std::string output_file = "/tmp/etw_events.csv";
    
    if (argc > 1) {
        output_file = argv[1];
    }
    
    ETWConsumer consumer(output_file);
    
    // Start consumer
    std::thread consumer_thread([&consumer]() {
        consumer.start();
    });
    
    // Run for 10 seconds
    std::this_thread::sleep_for(std::chrono::seconds(10));
    
    consumer.stop();
    consumer_thread.join();
    
    std::cout << "ETW Consumer finished" << std::endl;
    return 0;
}
