package dev.lazy.examples.spring;

import java.util.Map;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@SpringBootApplication
public class SpringExampleApplication {
  public static void main(String[] args) {
    SpringApplication.run(SpringExampleApplication.class, args);
  }
}

@RestController
class HelloController {
  @GetMapping("/")
  Map<String, String> hello() {
    return Map.of(
      "app", "spring",
      "message", "Hello from Spring Boot",
      "lazyUrl", System.getenv().getOrDefault("LAZY_URL", "")
    );
  }
}
